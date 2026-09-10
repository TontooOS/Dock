//! Dock pins persisted per user via CoreData.
//!
//! The store lives at `/Users/<user>/Library/Preferences/com.tontoo.dock/`
//! (entity `DockPin`: `bundle_id` + `position`), so every user has their own
//! dock order. Pin order is dock order; newly pinned apps always go last
//! (far right). A global version counter lets the running dock rebuild its
//! row live when pins change (see `version` / `bump`).
//!
//! Every function degrades gracefully: when CoreData is unavailable the
//! caller falls back to unpinned defaults and nothing is saved.

use std::sync::atomic::{AtomicUsize, Ordering};

use crate::apps::AppItem;

/// Bundle id owning the dock pin store.
pub const DOCK_BUNDLE_ID: &str = "com.tontoo.dock";

/// Entity holding one pinned app.
const ENTITY: &str = "DockPin";

/// Entity holding the single initialization marker, so "user removed every
/// pin" (empty dock) is distinct from "never saved" (defaults apply).
const META_ENTITY: &str = "DockMeta";

/// Stable object id of the marker.
const META_ID: &str = "meta";

/// Bumped on every successful pin change; the dock watches it to rebuild live.
static VERSION: AtomicUsize = AtomicUsize::new(0);

/// Current pins version. The dock polls this and rebuilds on change.
pub fn version() -> usize {
  VERSION.load(Ordering::Relaxed)
}

/// Announce a pin change to the running dock.
pub fn bump() {
  VERSION.fetch_add(1, Ordering::Relaxed);
}

fn open_container() -> Option<crate::CoreData::PersistentContainer> {
  crate::CoreData::PersistentContainer::new_with_bundle(
    DOCK_BUNDLE_ID.to_string(),
    crate::CoreData::StoreType::Fico,
  )
  .map_err(|err| {
    if std::env::var("DOCK_DEBUG").is_ok() {
      eprintln!("[dock] coredata unavailable: {err}");
    }
  })
  .ok()
}

/// Ordered pinned apps, resolved against `all` (stale entries dropped).
/// `None` when nothing was ever saved or the store is unreadable.
/// An explicitly emptied dock yields `Some(vec![])` via the meta marker.
pub fn load(all: &[AppItem]) -> Option<Vec<AppItem>> {
  let mut container = open_container()?;
  let ctx = container.view_context();
  // Initialization marker: present after the first save, even for an
  // explicitly emptied dock.
  ctx.object(META_ENTITY, META_ID).ok()?;
  let mut rows = ctx.fetch_all(ENTITY).ok()?;
  rows.sort_by_key(|obj| obj.get_i64("position").unwrap_or(i64::MAX));
  let mut pins = Vec::new();
  for row in &rows {
    let Some(bundle_id) = row.get_str("bundle_id") else {
      continue;
    };
    if let Some(item) = all.iter().find(|app| app.bundle_id == bundle_id) {
      pins.push(item.clone());
    }
  }
  Some(pins)
}

/// Rewrite the pin store in order. Returns `false` (and bumps nothing)
/// when CoreData is unavailable.
pub fn save(pins: &[AppItem]) -> bool {
  let mut container = match open_container() {
    Some(container) => container,
    None => return false,
  };
  let mut ctx = container.view_context();
  let stored = match ctx.fetch_all(ENTITY) {
    Ok(stored) => stored,
    Err(err) => {
      eprintln!("[dock] pins save failed (fetch): {err}");
      return false;
    }
  };
  for obj in &stored {
    if let Err(err) = ctx.delete(&obj.object_id) {
      eprintln!("[dock] pins save failed (delete): {err}");
      return false;
    }
  }
  for (position, item) in pins.iter().enumerate() {
    let mut obj = ctx.create(ENTITY);
    obj.set("bundle_id", item.bundle_id.as_str());
    obj.set("position", position as i32);
    if let Err(err) = ctx.save_object(obj) {
      eprintln!("[dock] pins save failed (insert): {err}");
      return false;
    }
  }
  let meta = crate::CoreData::ManagedObject::with_id(META_ENTITY, META_ID);
  if let Err(err) = ctx.save_object(meta) {
    eprintln!("[dock] pins save failed (meta): {err}");
    return false;
  }
  if let Err(err) = ctx.save() {
    eprintln!("[dock] pins save failed (commit): {err}");
    return false;
  }
  bump();
  true
}

/// Whether `bundle_id` is currently pinned (resolves against installed apps).
pub fn is_pinned(bundle_id: &str) -> bool {
  let all = crate::apps::load_all();
  load(&all)
    .map(|pins| pins.iter().any(|pin| pin.bundle_id == bundle_id))
    .unwrap_or(false)
}
/// Pin `item` (appended far right) or remove it when already pinned.
/// Returns the new state (`true` = pinned). `false` without change when
/// the app is unknown or the store is unavailable.
pub fn toggle(item: &AppItem) -> bool {
  if item.bundle_path.is_none() {
    return false;
  }
  let all = crate::apps::load_all();
  let mut pins = load(&all).unwrap_or_default();
  if pins.iter().any(|pin| pin.bundle_id == item.bundle_id) {
    pins.retain(|pin| pin.bundle_id != item.bundle_id);
    if save(&pins) {
      println!("[dock] removed {} from dock", item.display_name);
      return false;
    }
    return true;
  }
  let Some(full) = all.iter().find(|app| app.bundle_id == item.bundle_id) else {
    return false;
  };
  pins.push(full.clone());
  if save(&pins) {
    println!("[dock] pinned {} to dock", item.display_name);
    return true;
  }
  false
}

#[cfg(test)]
mod tests {
  use super::*;
  use std::path::PathBuf;

  static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

  struct TestEnv {
    dir: PathBuf,
    prefs: Option<std::ffi::OsString>,
    foreign: Option<std::ffi::OsString>,
    keyfile: Option<std::ffi::OsString>,
    _guard: std::sync::MutexGuard<'static, ()>,
  }

  impl TestEnv {
    fn setup(tag: &str) -> Self {
      let guard = LOCK.lock().unwrap();
      let dir =
        std::env::temp_dir().join(format!("dock-pins-test-{}-{tag}", std::process::id()));
      let _ = std::fs::remove_dir_all(&dir);
      std::fs::create_dir_all(&dir).unwrap();
      let prefs = std::env::var_os("TONTOO_PREFERENCES_ROOT");
      let foreign = std::env::var_os("TONTOO_COREDATA_ALLOW_FOREIGN");
      let keyfile = std::env::var_os("TONTOO_COREDATA_KEY_FILE");
      std::env::set_var("TONTOO_PREFERENCES_ROOT", &dir);
      std::env::set_var("TONTOO_COREDATA_ALLOW_FOREIGN", "1");
      std::env::set_var("TONTOO_COREDATA_KEY_FILE", dir.join("keyfile"));
      Self {
        dir,
        prefs,
        foreign,
        keyfile,
        _guard: guard,
      }
    }

    fn item(name: &str, bundle_id: &str) -> AppItem {
      let (color, symbol) = crate::apps::fallback_style(name);
      AppItem {
        display_name: name.to_string(),
        bundle_id: bundle_id.to_string(),
        bundle_path: Some(PathBuf::from(format!("/Applications/{name}.app"))),
        icon_path: None,
        color,
        symbol,
        demo_cmd: None,
      }
    }
  }

  impl Drop for TestEnv {
    fn drop(&mut self) {
      match &self.prefs {
        Some(v) => std::env::set_var("TONTOO_PREFERENCES_ROOT", v),
        None => std::env::remove_var("TONTOO_PREFERENCES_ROOT"),
      }
      match &self.foreign {
        Some(v) => std::env::set_var("TONTOO_COREDATA_ALLOW_FOREIGN", v),
        None => std::env::remove_var("TONTOO_COREDATA_ALLOW_FOREIGN"),
      }
      match &self.keyfile {
        Some(v) => std::env::set_var("TONTOO_COREDATA_KEY_FILE", v),
        None => std::env::remove_var("TONTOO_COREDATA_KEY_FILE"),
      }
      let _ = std::fs::remove_dir_all(&self.dir);
    }
  }

  #[test]
  fn pins_roundtrip_keeps_order() {
    let _env = TestEnv::setup("roundtrip");
    // Nothing saved yet.
    let all = vec![
      TestEnv::item("Alpha", "com.test.alpha"),
      TestEnv::item("Beta", "com.test.beta"),
    ];
    assert!(load(&all).is_none());
    // Save out of order; load returns pin order, stale ids dropped.
    let pins = vec![all[1].clone(), all[0].clone()];
    let before = version();
    assert!(save(&pins));
    assert_eq!(version(), before + 1);
    let back = load(&all).expect("pins stored");
    assert_eq!(back.len(), 2);
    assert_eq!(back[0].bundle_id, "com.test.beta");
    assert_eq!(back[1].bundle_id, "com.test.alpha");
  }

  #[test]
  fn empty_dock_stays_empty() {
    let _env = TestEnv::setup("empty");
    assert!(save(&[]));
    let back = load(&[]).expect("meta marker stored");
    assert!(back.is_empty());
  }
}
