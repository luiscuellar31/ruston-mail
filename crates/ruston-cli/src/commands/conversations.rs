//! Conversation commands. A conversation is named by its ID alone: core
//! looks search text up only as messages, so it cannot name a conversation.

use crate::cli::{ConversationsCmd, Ctx, ReadState, unix_now};
use crate::render;
use ruston_core::{Client, Result};

/// Lists, searches or reads conversations, or changes the ones named.
pub async fn run(ctx: &Ctx, cmd: ConversationsCmd) -> Result<()> {
    let client = Client::resume(&ctx.profile).await?;
    // Listing and reading show what they find; a change is made, then
    // reported, the same way for every command that makes one.
    let (ids, change) = match cmd {
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
            return Ok(());
        }
        ConversationsCmd::Search(args) => {
            let opts = crate::commands::search_options(&client, args.into()).await?;
            let convs = client.search_conversations(&opts).await?;
            render::conversations_list(ctx.json, convs.len() as u32, &convs);
            return Ok(());
        }
        ConversationsCmd::Read { id } => {
            let (conv, msgs) = client.read_conversation_each(&id).await?;
            render::conversation_read(ctx.json, &conv, &msgs);
            return Ok(());
        }
        ConversationsCmd::Move { ids, dest } => (ids, Change::Move { dest }),
        ConversationsCmd::Trash { ids } => (ids, Change::Trash),
        ConversationsCmd::Mark { state, folder, ids } => (ids, Change::Mark { state, folder }),
        ConversationsCmd::Star { ids } => (ids, Change::Star),
        ConversationsCmd::Unstar { ids } => (ids, Change::Unstar),
        ConversationsCmd::Snooze {
            ids,
            until,
            relative,
        } => {
            let until = snooze_time(until, relative, unix_now())?;
            (ids, Change::Snooze { until })
        }
        ConversationsCmd::Unsnooze { ids } => (ids, Change::Unsnooze),
    };
    change.apply(&client, &ids).await?;
    render::action_result(ctx.json, change.action(), &ids);
    Ok(())
}

/// What a command changes about the conversations it names.
enum Change {
    /// Moves them to a folder, named as the folder options take it.
    Move {
        dest: String,
    },
    Trash,
    /// Marks them read or unread where `folder` holds them.
    Mark {
        state: ReadState,
        folder: String,
    },
    Star,
    Unstar,
    /// Hides them until a Unix time.
    Snooze {
        until: i64,
    },
    Unsnooze,
}

impl Change {
    /// The command's own name, which the report gives.
    fn action(&self) -> &'static str {
        match self {
            Change::Move { .. } => "move",
            Change::Trash => "trash",
            Change::Mark { .. } => "mark",
            Change::Star => "star",
            Change::Unstar => "unstar",
            Change::Snooze { .. } => "snooze",
            Change::Unsnooze => "unsnooze",
        }
    }

    /// Asks Proton to make the change to `ids`, finding a folder it names
    /// first.
    async fn apply(&self, client: &Client, ids: &[String]) -> Result<()> {
        match self {
            Change::Move { dest } => {
                let dest = client.folder_id(dest).await?;
                client.move_conversations(ids, &dest).await
            }
            Change::Trash => client.trash_conversations(ids).await,
            Change::Mark { state, folder } => {
                let folder = client.folder_id(folder).await?;
                client
                    .mark_conversations_read(ids, state.as_bool(), &folder)
                    .await
            }
            Change::Star => client.star_conversations(ids, true).await,
            Change::Unstar => client.star_conversations(ids, false).await,
            Change::Snooze { until } => client.snooze_conversations(ids, *until).await,
            Change::Unsnooze => client.unsnooze_conversations(ids).await,
        }
    }
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

    #[test]
    fn each_change_is_reported_under_its_commands_name() {
        use clap::CommandFactory;

        let cli = crate::cli::Cli::command();
        let conversations = cli.find_subcommand("conversations").unwrap();
        let mark = Change::Mark {
            state: ReadState::Read,
            folder: "all".into(),
        };
        for (change, name) in [
            (Change::Move { dest: "x".into() }, "move"),
            (Change::Trash, "trash"),
            (mark, "mark"),
            (Change::Star, "star"),
            (Change::Unstar, "unstar"),
            (Change::Snooze { until: 1 }, "snooze"),
            (Change::Unsnooze, "unsnooze"),
        ] {
            assert_eq!(change.action(), name);
            assert!(conversations.find_subcommand(name).is_some(), "{name}");
        }
    }
}
