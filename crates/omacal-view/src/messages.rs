//! Fixed user-facing sentences that `errors::user_facing` safelists and the
//! write bodies raise. Moved here so the allowlist and the messages it names
//! live together, reachable without the modules that emit them.

// Referenced by `errors`; each is a fixed literal with no secret in it.

pub const LIST_NAME_TAKEN: &str = "there is already a list called ";

/// `list_name`'s three refusals. Named so they can be allow-listed: each
/// says what to do about a name the user just typed, and OPAQUE for a
/// 61-character list name would report a sync fault for a typo.
///
/// **The collision one names the list**, and so is a `SAFE_PREFIXES` entry
/// rather than an exact one: the trailing detail is a list name the user or
/// their server chose, which is the "variable, benign" case that list is for.
/// Naming it was deliberate — `lists_on_this_device_are_created_renamed_and_
/// deleted` asserts the name appears — and two lists whose collision is with
/// one the user cannot see is exactly when the name earns its place.
pub const LIST_NEEDS_A_NAME: &str = "a list needs a name";

pub const LIST_NAME_TOO_LONG: &str = "a list name can be up to 60 characters";

pub const NOT_A_TASK_LIST: &str =
    "that is not a task list you can add to — `omacal tasks` names the lists in each row";

/// A move that did not happen: the copy was refused, or the original would
/// not come off its list and the copy was taken back.
pub const TASK_NOT_MOVED: &str =
    "The task could not be moved to that list — it is still on the old one.";

/// A move that half happened, which the next sync would otherwise show as a
/// second copy with no word of why. Worded as `events::MOVED_NOT_REMOVED` is,
/// for the same situation.
pub const TASK_ON_BOTH_LISTS: &str =
    "The task is now on the list you chose, but OmaCal could not take it off the \
     old one — it is on both. Delete the copy on the old list.";

pub const TASK_CHANGED_ON_SERVER: &str =
    "That task changed on the server since it was loaded — sync and try again";

pub const TASK_GONE: &str = "that task is no longer here";

pub const TASK_NEEDS_A_TITLE: &str = "a task needs a title";

/// A tick on a repeating task (#145). OmaCal can only complete the whole
/// VTODO, and `STATUS:COMPLETED` on a series ends it for every client, so it
/// refuses before writing and says where the tick can be made instead.
pub const TASK_REPEATS: &str =
    "This task repeats, and OmaCal cannot complete a single occurrence yet — \
     completing it here would end the whole series, so nothing was changed. \
     Tick it off in the app that made it.";

/// What a create reports when Google succeeded and the local half did not.
///
/// A fixed literal, safelisted verbatim in `errors.rs`: the one thing this
/// message must never do is look like a failed create, because the natural
/// answer to a failed create is to try again — and trying again mails every
/// guest a second invitation. `App.svelte` recognises the sentence and runs
/// its ordinary post-write sync instead of stopping, which is the heal the
/// sentence promises.
pub const CREATED_NOT_STORED: &str =
    "The event was created on Google, but OmaCal could not record it locally. \
     The next sync will bring it in — do not create it again.";

/// What an edit, move or delete reports when the server took it and the local
/// half did not (a CalDAV resync after the write failed).
///
/// [`CREATED_NOT_STORED`]'s rule for changes: a fixed literal, safelisted
/// verbatim in `errors.rs`, because reported as an ordinary failure it reads
/// as a refusal, and the app undoes on screen a change that happened. The UI
/// recognises it, keeps the change drawn, and the next sync brings it in.
pub const SAVED_NOT_STORED: &str =
    "The change was saved, but OmaCal could not record it locally. \
     The next sync will bring it in.";

/// What the user is told when a guest-list change loses a race.
///
/// A named constant because it is asserted in two places that must not drift:
/// the test that pins it, and `errors.rs`'s `SAFE_EXACT` allowlist, which shows
/// a message verbatim only if it matches one there exactly.
pub const CONFLICT_GUESTS: &str =
    "somebody else changed this event while you were editing it, so the guest list was not \
     saved — close the form, let it refresh, and make the change again";

/// What a move earns when the event is one occurrence of a series.
///
/// Google's `events.move` takes an event id, and for a recurring event that is
/// the master: there is no request that re-files one occurrence and leaves its
/// siblings behind, and CalDAV is the same — the whole `VCALENDAR` resource is
/// one file in one collection. So this refuses rather than quietly moving more
/// than was asked for. Allowlisted in `errors.rs`: the user can act on it, and
/// the action is the scope choice already on screen.
pub const MOVE_ONE_OCCURRENCE: &str =
    "one occurrence cannot be moved on its own — choose All events to move the whole series";

/// What a move earns when the destination belongs to another account.
///
/// Not a limitation to be lifted later: across accounts there is no move, only
/// a copy and a delete. The event would take a new id, every guest would be
/// invited afresh, and every RSVP already given would be discarded — the exact
/// outcome that made "delete it and make it again" the workaround this feature
/// exists to remove. Refused with the reason rather than performed silently.
pub const MOVE_ACROSS_ACCOUNTS: &str =
    "an event can only be moved between calendars on the same account";

/// What a CalDAV move reports when the copy landed and the original would not
/// go — the one partial outcome the PUT-then-DELETE order can produce.
///
/// [`CREATED_NOT_STORED`]'s reasoning, for the other write: the thing this
/// message must never do is read as a failed move, because the answer to a
/// failed move is to try it again, and trying again puts a second copy
/// nowhere useful. It says where both copies are and which one to delete,
/// because that is a thing a person can actually do. Safelisted verbatim in
/// `errors.rs`; the underlying transport error goes to the log.
pub const MOVED_NOT_REMOVED: &str =
    "The event is now on the calendar you chose, but omacal could not remove it \
     from the old one — it is on both. Delete the copy on the old calendar.";

/// The 412 an etag-guarded write answers with, worded as its task twin
/// `tasks::TASK_CHANGED_ON_SERVER` is. Both name the one fix there is —
/// sync, then save again — and until 2026-09-23 only the task one was
/// allow-listed, so the same conflict told a task user what to do and left
/// an event user reading "Sync failed. See the application log for details."
pub const EVENT_CHANGED_ON_SERVER: &str =
    "That event changed on the server since it was loaded — sync and try again";

/// The write paths' two refusals before anything leaves the machine.
pub const EVENT_NOT_SYNCED_YET: &str =
    "this event has not finished syncing; try again in a moment";

/// A fixed literal, in `errors::SAFE_EXACT`: a failed write must not put a
/// filesystem path or an OS error into a message the user reads.
pub const EXPORT_FAILED: &str =
    "Could not write that file. Check the folder and try again.";

/// The chooser went away without answering — a shape that should not happen,
/// and one the user can act on if it does.
pub const EXPORT_DISMISSED: &str = "The file chooser closed without an answer.";

/// The row went out from under the click — a sync landed between opening the
/// details card and pressing Export.
pub const EXPORT_GONE: &str = "That event is no longer here.";

/// What the user reads when no browser could be started. Fixed literal, in
/// `errors::SAFE_EXACT`: a launcher failure carries no secret, and the
/// opaque fallback is what cost issue #1's reporter their afternoon — the
/// one fact that pointed at the cause was the one thing withheld.
pub const BROWSER_FAILED: &str =
    "Could not open a browser. Open your browser once and try again.";
