//! Conversation commands.
//!
//! Conversation references are treated as literal conversation IDs: the core
//! `resolve_ref` helper resolves to *message* IDs, and there is no
//! conversation-level resolver, so free-text resolution is not available here.

use crate::cli::{ConversationsCmd, Ctx};
use crate::render;
use ruston_core::Result;

pub async fn run(ctx: &Ctx, cmd: ConversationsCmd) -> Result<()> {
    let client = crate::commands::resume(&ctx.profile).await?;
    match cmd {
        ConversationsCmd::List {
            folder,
            page,
            page_size,
            unread,
        } => {
            let (total, convs) = client
                .list_conversations(&folder, page, page_size, unread)
                .await?;
            render::conversations_list(ctx.json, total, &convs);
        }
        ConversationsCmd::Search(args) => {
            let opts = args.into();
            let convs = client.search_conversations(&opts).await?;
            render::conversations_list(ctx.json, convs.len() as u32, &convs);
        }
        ConversationsCmd::Read { id } => {
            let (conv, msgs) = client.read_conversation_each(&id).await?;
            render::conversation_read(ctx.json, &conv, &msgs);
        }
        ConversationsCmd::Move { ids, dest } => {
            client.move_conversations(&ids, &dest).await?;
            render::action_result(ctx.json, "move", &ids);
        }
        ConversationsCmd::Trash { ids } => {
            client.trash_conversations(&ids).await?;
            render::action_result(ctx.json, "trash", &ids);
        }
        ConversationsCmd::Mark { state, folder, ids } => {
            client
                .mark_conversations_read(&ids, state.as_bool(), &folder)
                .await?;
            render::action_result(ctx.json, "mark", &ids);
        }
        ConversationsCmd::Star { ids } => {
            client.star_conversations(&ids, true).await?;
            render::action_result(ctx.json, "star", &ids);
        }
        ConversationsCmd::Unstar { ids } => {
            client.star_conversations(&ids, false).await?;
            render::action_result(ctx.json, "unstar", &ids);
        }
        ConversationsCmd::Snooze {
            ids,
            until,
            relative,
        } => {
            let ts = snooze_time(until, relative.as_deref(), now_unix())?;
            client.snooze_conversations(&ids, ts).await?;
            render::action_result(ctx.json, "snoozed", &ids);
        }
        ConversationsCmd::Unsnooze { ids } => {
            client.unsnooze_conversations(&ids).await?;
            render::action_result(ctx.json, "unsnoozed", &ids);
        }
    }
    Ok(())
}

fn now_unix() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// When a snooze ends: `--until` as given, which wins over `--in`, or `now`
/// plus the `--in` duration.
pub(crate) fn snooze_time(until: Option<i64>, relative: Option<&str>, now: i64) -> Result<i64> {
    match (until, relative) {
        (Some(until), _) => Ok(until),
        (None, Some(duration)) => now
            .checked_add(parse_duration_secs(duration)?)
            .ok_or_else(|| ruston_core::Error::Other(format!("duration too long: {duration}"))),
        (None, None) => Err(ruston_core::Error::Other(
            "provide --until <unix> or --in <duration>".into(),
        )),
    }
}

/// Parse `30m`, `2h`, `1d`, `45s` (or bare seconds) into a positive number
/// of seconds.
fn parse_duration_secs(s: &str) -> Result<i64> {
    let s = s.trim();
    let split = s.find(|c: char| c.is_alphabetic()).unwrap_or(s.len());
    let (num, unit) = s.split_at(split);
    let n: i64 = num
        .parse()
        .map_err(|_| ruston_core::Error::Other(format!("bad duration: {s}")))?;
    if n <= 0 {
        return Err(ruston_core::Error::Other(format!(
            "bad duration: {s} (must be more than zero)"
        )));
    }
    let mult = match unit {
        "" | "s" => 1,
        "m" => 60,
        "h" => 3600,
        "d" => 86400,
        other => {
            return Err(ruston_core::Error::Other(format!(
                "bad duration unit: {other}"
            )));
        }
    };
    n.checked_mul(mult)
        .ok_or_else(|| ruston_core::Error::Other(format!("duration too long: {s}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snooze_durations_are_positive_and_never_wrap() {
        for (duration, seconds) in [("30m", 1800), ("1d", 86_400), ("+5m", 300), ("45", 45)] {
            assert_eq!(
                parse_duration_secs(duration).unwrap(),
                seconds,
                "{duration}"
            );
        }
        for duration in [
            "-5m",
            "0",
            "0d",
            "106751991167301d",
            "9223372036854775807h",
            "5w",
        ] {
            assert!(parse_duration_secs(duration).is_err(), "{duration}");
        }
    }

    #[test]
    fn snooze_time_keeps_until_first_and_never_wraps() {
        let now = 1_700_000_000;
        // --until wins, as it always has, without reading --in.
        assert_eq!(snooze_time(Some(42), Some("garbage"), now).unwrap(), 42);
        assert_eq!(snooze_time(None, Some("1h"), now).unwrap(), now + 3600);
        assert!(snooze_time(None, Some("9223372036854775807"), now).is_err());
        assert!(snooze_time(None, None, now).is_err());
    }
}
