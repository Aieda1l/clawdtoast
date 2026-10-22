//! Fallback for desktops where no overlay window is possible (GNOME on Wayland has no
//! layer-shell): each card becomes a freedesktop desktop notification. There is no Clawd and the
//! desktop owns timing and placement. Uses `notify-send` or `gdbus` rather than a D-Bus library,
//! so the binary stays small.
#![cfg_attr(not(target_os = "linux"), allow(dead_code))]

use crate::config::Config;
use crate::core::Card;
use crate::log;
use crate::protocol;
use std::collections::HashMap;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// Shows each card from stdin as a notification, until the daemon is done with us.
pub fn run() {
    let cfg = Config::load();
    let timeout_ms = (cfg.duration_secs * 1000.0).round() as u32;
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || crate::overlay::read_cards(|card| tx.send(card).is_ok()));
    let mut ids: HashMap<String, (u32, u64, bool)> = HashMap::new();
    let mut last = Instant::now();
    loop {
        let wall = protocol::now_ms();
        for (id, expiry, _) in ids.values() {
            if *expiry <= wall {
                close(*id);
            }
        }
        ids.retain(|_, (_, expiry, _)| *expiry > wall);
        let mut wait = ids
            .values()
            .map(|(_, expiry, _)| Duration::from_millis(expiry.saturating_sub(wall)))
            .min()
            .unwrap_or(crate::anim::IDLE_EXIT);
        let persistent = ids.values().any(|(_, _, persistent)| *persistent);
        if !persistent {
            wait = wait.min(crate::anim::IDLE_EXIT.saturating_sub(last.elapsed()));
        }
        let card = match rx.recv_timeout(wait) {
            Ok(card) => card,
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                for (id, _, _) in ids.values() {
                    close(*id);
                }
                return;
            }
            Err(_) => {
                if !persistent && last.elapsed() >= crate::anim::IDLE_EXIT {
                    return;
                }
                continue;
            }
        };
        last = Instant::now();
        let key = card.slot();
        let previous = ids.get(&key).map_or(0, |(id, _, _)| *id);
        if card.dismiss {
            if previous != 0 {
                close(previous);
            }
            ids.remove(&key);
            continue;
        }
        if (card.silent && previous == 0) || card.expired(protocol::now_ms()) {
            continue;
        }
        let timeout = if card.expires_at > 0 {
            card.expires_at
                .saturating_sub(protocol::now_ms())
                .min(u32::MAX as u64) as u32
        } else {
            timeout_ms
        };
        if let Some(id) = deliver(&card, timeout.max(1), previous) {
            // Keep bounded attention notifications addressable until their deadline.
            let expiry = if card.expires_at > 0 {
                card.expires_at
            } else {
                protocol::now_ms() + u64::from(timeout)
            };
            ids.insert(key, (id, expiry, card.expires_at > 0));
        } else {
            log::line("notify: neither notify-send nor gdbus could deliver the notification");
        }
    }
}

fn deliver(card: &Card, timeout_ms: u32, previous: u32) -> Option<u32> {
    for (prog, mut args) in commands(card, timeout_ms) {
        if prog == "notify-send" {
            args.insert(0, "--print-id".into());
            if previous != 0 {
                args.insert(0, format!("--replace-id={previous}"));
            }
        } else {
            args[6] = previous.to_string();
        }
        if let Ok(output) = Command::new(prog)
            .args(args)
            .stdin(Stdio::null())
            .stderr(Stdio::null())
            .output()
        {
            if output.status.success() {
                let text = String::from_utf8_lossy(&output.stdout);
                return Some(notification_id(&text).unwrap_or(0));
            }
        }
    }
    None
}

fn notification_id(output: &str) -> Option<u32> {
    output.trim().parse().ok().or_else(|| {
        output
            .trim()
            .strip_prefix("(uint32 ")?
            .trim_end_matches([')', ','])
            .parse()
            .ok()
    })
}

fn close(id: u32) {
    if id == 0 {
        return;
    }
    // Best effort: some desktops ignore expiration/replacement requests. The
    // finite timeout remains the fallback when gdbus is unavailable.
    let _ = Command::new("gdbus")
        .args([
            "call",
            "--session",
            "--dest=org.freedesktop.Notifications",
            "--object-path=/org/freedesktop/Notifications",
            "--method=org.freedesktop.Notifications.CloseNotification",
            &id.to_string(),
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
}

/// Commands to try in order for `card`. A new card for the same session replaces the previous
/// notification where the daemon supports a sync hint (GNOME, KDE, dunst).
fn commands(card: &Card, timeout_ms: u32) -> Vec<(&'static str, Vec<String>)> {
    let tag = format!("clawdtoast-{}", card.slot());
    let title = card.title();
    let notify_send = vec![
        "--app-name=clawdtoast".to_string(),
        format!("--expire-time={timeout_ms}"),
        format!("--hint=string:x-canonical-private-synchronous:{tag}"),
        format!("--hint=string:x-dunst-stack-tag:{tag}"),
        "--hint=boolean:suppress-sound:true".into(),
        "--".to_string(),
        title.clone(),
        card.body.clone(),
    ];
    let gdbus = vec![
        "call".into(),
        "--session".into(),
        "--dest=org.freedesktop.Notifications".into(),
        "--object-path=/org/freedesktop/Notifications".into(),
        "--method=org.freedesktop.Notifications.Notify".into(),
        "clawdtoast".into(),
        "0".into(),
        "''".into(),
        gvariant_str(&title),
        gvariant_str(&card.body),
        "[]".into(),
        format!(
            "{{'x-canonical-private-synchronous': <{}>, 'suppress-sound': <true>}}",
            gvariant_str(&tag)
        ),
        timeout_ms.to_string(),
    ];
    vec![("notify-send", notify_send), ("gdbus", gdbus)]
}

/// A GVariant text-format string literal (gdbus parses its arguments as GVariant text).
fn gvariant_str(s: &str) -> String {
    let mut out = String::from("'");
    for c in s.chars() {
        match c {
            '\'' | '\\' => {
                out.push('\\');
                out.push(c);
            }
            '\n' => out.push_str("\\n"),
            c => out.push(c),
        }
    }
    out.push('\'');
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::Kind;

    fn card(body: &str) -> Card {
        Card {
            session_id: "abcd1234".into(),
            kind: Kind::Done,
            project: "proj".into(),
            body: body.into(),
            badge: None,
            ts: 0,
            pid: 0,
            app: String::new(),
            agent: String::new(),
            pending_count: 0,
            expires_at: 0,
            silent: false,
            dismiss: false,
        }
    }

    #[test]
    fn gvariant_escapes_quotes_backslashes_and_newlines() {
        assert_eq!(gvariant_str(r"it's a \ test"), r"'it\'s a \\ test'");
        assert_eq!(gvariant_str("a\nb"), r"'a\nb'");
    }

    #[test]
    fn native_response_ids_are_parsed_without_accepting_arbitrary_output() {
        assert_eq!(notification_id("42\n"), Some(42));
        assert_eq!(notification_id("(uint32 123,)\n"), Some(123));
        assert_eq!(notification_id("warning: 123"), None);
        assert_eq!(notification_id(""), None);
    }

    #[test]
    fn notify_send_gets_title_body_timeout_and_a_replace_hint() {
        let cmds = commands(&card("Fixed it."), 5000);
        let (prog, args) = &cmds[0];
        assert_eq!(*prog, "notify-send");
        assert!(args.contains(&"--expire-time=5000".to_string()));
        assert!(args
            .iter()
            .any(|a| a == "--hint=string:x-canonical-private-synchronous:clawdtoast-abcd1234"));
        // Title and body come last, after `--`, so text starting with a dash is not an option.
        assert_eq!(&args[args.len() - 3..], ["--", "Done · proj", "Fixed it."]);
    }

    #[test]
    fn gdbus_args_are_quoted_for_the_variant_parser() {
        let cmds = commands(&card("it's done"), 3000);
        let (prog, args) = &cmds[1];
        assert_eq!(*prog, "gdbus");
        assert!(args.contains(&r"'it\'s done'".to_string()));
        assert_eq!(args.last().unwrap(), "3000");
    }
}
