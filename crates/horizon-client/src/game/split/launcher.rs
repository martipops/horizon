//! The launcher's offline names as splitscreen profiles: one list. The launcher hands the game
//! every name with its look (`HORIZON_PROFILES`) and itself (`HORIZON_LAUNCHER_EXE`); a joining
//! player picks from them, and a name made on the picker is kept by the launcher too
//! (`add-offline`), so its list stays the one list: the launcher manages them. Each name keeps
//! its options in its profile folder. Without the launcher (a development run) the folders are
//! the list.
use horizon_client::auth::AuthSession;
use std::{path::PathBuf, sync::Arc};

/// An offline name and its skin, as the launcher keeps them.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct Profile {
    pub(super) name: String,
    pub(super) skin: Option<PathBuf>,
    pub(super) slim: bool,
}

/// The launcher's offline names (`HORIZON_PROFILES`), none when the launcher didn't start the
/// game.
pub(super) fn profiles() -> Option<Vec<Profile>> {
    parse(&std::env::var("HORIZON_PROFILES").ok()?)
}

fn parse(json: &str) -> Option<Vec<Profile>> {
    let list: Vec<serde_json::Value> = serde_json::from_str(json).ok()?;
    Some(
        list.iter()
            .filter_map(|p| {
                Some(Profile {
                    name: p.get("name")?.as_str()?.to_owned(),
                    skin: p.get("skin").and_then(|s| s.as_str()).map(PathBuf::from),
                    slim: p.get("model").and_then(|m| m.as_str()) == Some("slim"),
                })
            })
            .collect(),
    )
}

/// The launcher's other signed-in accounts (`HORIZON_ACCOUNT_SESSIONS`: name, uuid and a session
/// file each), for splitscreen players to sign in with. Read at once: the files go when read and
/// are refused once a couple of minutes old.
pub(super) fn accounts() -> Vec<Arc<AuthSession>> {
    let Ok(json) = std::env::var("HORIZON_ACCOUNT_SESSIONS") else {
        return Vec::new();
    };
    let list: Vec<serde_json::Value> = serde_json::from_str(&json).unwrap_or_default();
    list.iter()
        .filter_map(|a| a.get("file")?.as_str())
        .filter_map(|file| match AuthSession::load(std::path::Path::new(file)) {
            Ok(session) => Some(Arc::new(session)),
            Err(e) => {
                log::warn!("Horizon: splitscreen: an account's session: {e}");
                None
            }
        })
        .collect()
}

/// One of the launcher's account commands (`add-offline`), run beside the game: its data is the
/// launcher's to change. Nothing without a launcher.
pub(super) fn run(args: &[&str]) {
    let Some(exe) = std::env::var_os("HORIZON_LAUNCHER_EXE") else {
        return;
    };
    let args: Vec<String> = std::iter::once("account")
        .chain(args.iter().copied())
        .map(str::to_owned)
        .collect();
    std::thread::spawn(
        move || match std::process::Command::new(&exe).args(&args).output() {
            Ok(out) if out.status.success() => {}
            Ok(out) => log::warn!(
                "Horizon: splitscreen: launcher {}: {}",
                args.join(" "),
                String::from_utf8_lossy(&out.stderr).trim()
            ),
            Err(e) => log::warn!("Horizon: splitscreen: launcher {}: {e}", args.join(" ")),
        },
    );
}

/// `name` plays with its launcher skin, where it has one (`skins::set_local_look`).
pub(super) fn wear(profiles: &[Profile], name: &str) {
    let Some(skin) = profiles
        .iter()
        .find(|p| p.name.eq_ignore_ascii_case(name))
        .and_then(|p| Some((p.skin.as_ref()?, p.slim)))
    else {
        return;
    };
    let uuid = horizon_protocol::crypto::offline_uuid(name);
    let set = std::fs::read(skin.0)
        .map_err(|e| e.to_string())
        .and_then(|png| horizon_client::skins::set_local_look(uuid, name, &png, skin.1));
    if let Err(e) = set {
        log::warn!("Horizon: splitscreen: {name}'s skin: {e}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_launcher_s_names_and_skins() {
        let json = r#"[{"name":"Bea","skin":"/s/bea.png","model":"slim","cape":null},
                       {"name":"Cal","skin":null,"model":null,"cape":null}, {"bad":1}]"#;
        let list = parse(json).unwrap();
        assert_eq!(list.len(), 2);
        assert_eq!(
            list[0],
            Profile {
                name: "Bea".into(),
                skin: Some("/s/bea.png".into()),
                slim: true
            }
        );
        assert_eq!((list[1].skin.as_ref(), list[1].slim), (None, false));
        assert!(parse("not json").is_none());
    }
}
