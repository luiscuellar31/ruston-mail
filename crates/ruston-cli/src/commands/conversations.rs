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
            let folder = client.folder_id(&folder).await?;
            let (total, convs) = client
                .list_conversations(&folder, page, page_size, unread)
                .await?;
            render::conversations_list(ctx.json, total, &convs);
        }
        ConversationsCmd::Search(args) => {
            let opts = crate::commands::search_options(&client, args.into()).await?;
            let convs = client.search_conversations(&opts).await?;
            render::conversations_list(ctx.json, convs.len() as u32, &convs);
        }
        ConversationsCmd::Read { id } => {
            let (conv, msgs) = client.read_conversation_each(&id).await?;
            render::conversation_read(ctx.json, &conv, &msgs);
        }
        ConversationsCmd::Move { ids, dest } => {
            let dest = client.folder_id(&dest).await?;
            client.move_conversations(&ids, &dest).await?;
            render::action_result(ctx.json, "move", &ids);
        }
        ConversationsCmd::Trash { ids } => {
            client.trash_conversations(&ids).await?;
            render::action_result(ctx.json, "trash", &ids);
        }
        ConversationsCmd::Mark { state, folder, ids } => {
            let folder = client.folder_id(&folder).await?;
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
            let ts = snooze_time(until, relative, now_unix())?;
            client.snooze_conversations(&ids, ts).await?;
            render::action_result(ctx.json, "snooze", &ids);
        }
        ConversationsCmd::Unsnooze { ids } => {
            client.unsnooze_conversations(&ids).await?;
            render::action_result(ctx.json, "unsnooze", &ids);
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

/// When a snooze ends: at `--until`, or `--in` seconds after `now`. The
/// arguments already refuse both together, neither, and a time gone by.
pub(crate) fn snooze_time(until: Option<i64>, relative: Option<i64>, now: i64) -> Result<i64> {
    match (until, relative) {
        (Some(until), _) => Ok(until),
        (None, Some(seconds)) => now
            .checked_add(seconds)
            .ok_or_else(|| ruston_core::Error::Other("duration too long".into())),
        (None, None) => Err(ruston_core::Error::Other(
            "provide --until <unix> or --in <duration>".into(),
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snooze_time_never_wraps() {
        let now = 1_700_000_000;
        assert_eq!(
            snooze_time(Some(1_800_000_000), None, now).unwrap(),
            1_800_000_000
        );
        assert_eq!(snooze_time(None, Some(3600), now).unwrap(), now + 3600);
        assert!(snooze_time(None, Some(i64::MAX), now).is_err());
        assert!(snooze_time(None, None, now).is_err());
    }
}
