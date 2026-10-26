//! The Telegram connector: the user talks to Shodh from their phone, and, if they allow
//! it, a few other people and groups ask questions of one workspace.
//!
//! The connection is the Telegram Bot API with long polling (`getUpdates`) from this
//! computer: no public URL, no server. The bot token comes from @BotFather and stays in
//! the OS credential store; it is part of every Bot API URL, so no URL is ever logged or
//! put in an error ([`api`]).
//!
//! - [`token`]: the bot token, checked and never printed.
//! - [`api`]: the Bot API calls, Telegram's errors (rate limits, conflicts, migrations).
//! - [`updates`]: what an update says (a message, the bot added to or removed from a
//!   group); whether a group message is addressed to the bot.
//! - [`link`]: one-time codes that link the owner's Telegram account, or invite someone.
//! - [`access`]: who is answered how. The owner's private chat gets the whole library and
//!   their memories; allowed people and groups get one workspace, never memories; anyone
//!   else is refused. Decided by the chat, never by what a message says.
//! - [`format`]: Telegram's HTML, the 4 096-character limit, citations by title and page.
//! - [`answer`]: the owner's prompt (library, memories, the chat so far, no tools).
//! - [`respond`]: one message start to finish, cheapest checks first.
//! - [`poll`]: the long-polling loop, its checkpoint, Telegram's rate limits, stopping.

pub mod access;
pub mod answer;
pub mod api;
pub mod format;
pub mod link;
pub mod poll;
pub mod respond;
pub mod token;
pub mod updates;

pub use tokio_util::sync::CancellationToken;
