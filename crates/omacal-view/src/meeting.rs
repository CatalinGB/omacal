//! Recognising a joinable meeting link in an event's fields.
//!
//! Moved here from `upcoming.rs`/`browser.rs` so the view assembly (the
//! "combine identical events" grouping decides whether a duplicated conference
//! is the same meeting) can use it without pulling in a browser launcher. The
//! `is_zoom_host`/`ZOOM_HOSTS` pair is the pure half of `browser.rs`'s Zoom
//! protocol work.

/// `zoom.us` alone left out the many domains Zoom actually issues under, and
/// the widget's `ZOOM_HOSTS` in `MeetingPresence.mjs` (#119).
pub const ZOOM_HOSTS: &[&str] = &["zoom.us", "zoom.com", "zoom-x.de", "zoomgov.com", "zoom.com.cn"];

/// Whether `host` is Zoom or a subdomain of one of its domains.
pub fn is_zoom_host(host: &str) -> bool {
    let h = host.to_ascii_lowercase();
    ZOOM_HOSTS.iter().any(|z| h == *z || h.ends_with(&format!(".{z}")))
}

/// The meeting providers a link may name. `teams.microsoft.com` and the
/// consumer `teams.live.com`, GCC High and DoD (`gov.`/`dod.teams.microsoft.us`)
/// and 21Vianet China (`teams.microsoftonline.cn`). `webexgov.us` is Webex for
/// Government's own domain. The JS twin is `TEAMS_HOSTS` in
/// `MeetingPresence.mjs`; keep the two in step.
const MEETING_PROVIDERS: &[&str] = &[
    "meet.google.com",
    "teams.microsoft.com", "teams.cloud.microsoft", "teams.live.com",
    "teams.microsoft.us", "teams.microsoftonline.cn",
    "webex.com", "webexgov.us", "meet.jit.si",
];

/// Walks every URL in `text`, in order, and returns the first one whose host
/// is a recognised meeting provider — the shared scan behind
/// [`location_meeting_url`] and [`conference_join_url`]'s description half,
/// and the Rust twin of `ui/src/lib/location.ts`'s `meetingUrl`.
///
/// The scheme search is case-insensitive — matching the `i` flag on the TS
/// twin's regex — and takes whichever scheme's match sits earlier in the
/// text. `rest.find("https://").or_else(|| rest.find("http://"))` looks
/// equivalent but is not: `.find` searches the whole remainder for each
/// scheme independently, so a *later* `https://` still wins over an
/// *earlier* `http://` whenever both are present, and the loop then advances
/// past that later match — silently dropping the earlier URL for good
/// rather than revisiting it next iteration.
pub fn first_recognised_url(text: &str) -> Option<String> {
    let mut rest = text;
    loop {
        let lower = rest.to_ascii_lowercase();
        let at = match (lower.find("https://"), lower.find("http://")) {
            (Some(a), Some(b)) => a.min(b),
            (Some(a), None) => a,
            (None, Some(b)) => b,
            (None, None) => return None,
        };
        let tail = &rest[at..];
        let end = tail
            .find(|c: char| {
                c.is_whitespace() || matches!(c, ',' | ';' | '<' | '>' | '"' | '\'')
            })
            .unwrap_or(tail.len());
        let url = tail[..end].trim_end_matches([')', ',', '.', ';', ':', '!', '?', ']']);

        // The hostname: after the scheme, before any path/query/fragment,
        // with userinfo and port stripped — enough of a parse for an
        // allow-list.
        let host = url
            .split_once("://")
            .and_then(|(_, after_scheme)| after_scheme.split(['/', '?', '#']).next())
            .map(|authority| {
                authority
                    .rsplit('@')
                    .next()
                    .unwrap_or("")
                    .split(':')
                    .next()
                    .unwrap_or("")
                    .to_ascii_lowercase()
            })
            .unwrap_or_default();

        if !host.is_empty()
            && (is_zoom_host(&host)
                || MEETING_PROVIDERS.iter().any(|p| host == *p || host.ends_with(&format!(".{p}"))))
        {
            return Some(url.to_string());
        }

        // Not a recognised link (or not parseable) — keep scanning past it.
        let consumed = at + end.max(1);
        if consumed >= rest.len() {
            return None;
        }
        rest = &rest[consumed..];
    }
}

/// The joinable meeting URL a location field holds, or `None`.
///
/// **Recognised providers only**, which is the whole of the restraint: a
/// location is as likely to hold a map pin or a venue's homepage as a
/// meeting, and a Join button that opens a restaurant is worse than none.
/// The trailing-punctuation trim matters more than it looks — a link
/// written into a sentence ("dial in at https://…/j/123.") otherwise
/// carries the full stop into the URL and 404s.
pub fn location_meeting_url(raw: Option<&str>) -> Option<String> {
    first_recognised_url(raw.unwrap_or("").trim())
}

/// The meeting to join, if any: a recognised link in `location`, and failing
/// that, one in `description`.
///
/// Plenty of invites put their join link only in the description ("Join Zoom
/// Meeting: https://…"), never in `location` — and this is the one place
/// that answer is decided. `open_conference`, this feed, and the two
/// `list`/`show` call sites in `cli.rs` all resolve through it, so the app,
/// the widget and the CLI never disagree about whether an event has a
/// meeting.
pub fn conference_join_url(location: Option<&str>, description: Option<&str>) -> Option<String> {
    location_meeting_url(location).or_else(|| first_recognised_url(description.unwrap_or("").trim()))
}
