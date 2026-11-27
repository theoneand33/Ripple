//! The Island's side of Nextcloud sync: starting a sync off the UI thread, one at a time per
//! kind, and merging its outcome into the settings when it ends.

use std::path::PathBuf;

use super::Undo;
use crate::config::Item;
use crate::island::Island;
use crate::nextcloud::{self, Kind, Outcome, ShelfOutcome, adopt, apply, in_folder, sync, sync_shelf};

/// One kind's sync: whether one runs, whether another was asked for meanwhile, and the last error.
#[derive(Default)]
pub struct SyncState {
    pub running: bool,
    again: bool,
    /// Shelf only: names dropped while a sync ran.
    pub dropped: Vec<String>,
    pub error: Option<String>,
}

/// Each kind's sync, by `Kind`.
#[derive(Default)]
pub struct Syncs([SyncState; Kind::ALL.len()]);

impl std::ops::Index<Kind> for Syncs {
    type Output = SyncState;
    fn index(&self, kind: Kind) -> &SyncState { &self.0[kind as usize] }
}

impl std::ops::IndexMut<Kind> for Syncs {
    fn index_mut(&mut self, kind: Kind) -> &mut SyncState { &mut self.0[kind as usize] }
}

impl Island {
    /// Saves an item's text, then syncs: in place when `editing` names one still there, keeping
    /// its Nextcloud link so the sync writes the change over it; else added afresh, so an edit
    /// of one gone since it began (done elsewhere, removed by a sync) isn't lost.
    pub(super) fn save_item(&mut self, kind: Kind, editing: Option<u64>, text: String) {
        self.save(|c| {
            let (items, _) = kind.lists(c);
            match editing.and_then(|key| items.iter_mut().find(|i| i.key == key)) { Some(i) => i.text = text, None => items.push(Item::new(text)) }
        });
        self.nextcloud_sync(kind);
    }

    /// Deletes an item, kept for Undo while its tab stays open, then syncs.
    pub(super) fn delete_item(&mut self, kind: Kind, key: u64) {
        let mut gone = None;
        self.save(|c| gone = nextcloud::remove(c, kind, key));
        if let Some((i, item)) = gone { self.undo = Some(Undo::Item(kind, i, item)) }
        self.nextcloud_sync(kind);
    }

    /// Syncs one kind with Nextcloud in the background; a request while one runs queues one more.
    pub fn nextcloud_sync(&mut self, kind: Kind) {
        if !self.cfg.nextcloud.enabled() { return }
        let Some(me) = self.me.clone() else { return };
        let state = &mut self.nextcloud[kind];
        if state.running { state.again = true; return }
        state.running = true;
        let account = self.cfg.nextcloud.account();
        let nc = self.cfg.nextcloud.clone();
        let (items, pending) = kind.lists(&mut self.cfg);
        let (mut items, mut pending) = (items.clone(), pending.clone());
        // Ids from another account mean nothing to this one: everything uploads as new. The saved
        // ids are only dropped once this account answers, so a half-typed URL loses nothing.
        let fresh = nc.synced_with != account;
        if fresh {
            for i in &mut items { i.remote = None }
            pending.clear();
        }
        let today = self.today;
        me.spawn(move || sync(&nc, kind, &items, &pending, today), move |s, out| s.nextcloud_synced(kind, &account, fresh, out));
    }

    fn nextcloud_synced(&mut self, kind: Kind, account: &str, fresh: bool, out: Outcome) {
        self.nextcloud[kind].running = false;
        let current = self.cfg.nextcloud.account() == account;
        self.nextcloud[kind].error = out.error.clone();
        // A new account that failed at once has nothing of ours: keep the old ids.
        let answered = !fresh || out.server.is_some() || !out.pushed.is_empty();
        // Otherwise the account changed while it ran, or a new one never answered.
        if current && answered {
            // The note the editor shows, to keep it open wherever the sync moves it.
            let open = (kind == Kind::Notes).then(|| self.cfg.notes.get(self.notes.index).map(|n| (n.key, n.text.clone()))).flatten();
            self.save(|c| {
                adopt(c, account);
                let (items, pending) = kind.lists(c);
                apply(items, pending, out);
            });
            if kind == Kind::Notes { self.notes_synced(open) }
        }
        // A changed account waits for the next sync asked of it, so no resync chases a URL that is still being typed.
        if std::mem::take(&mut self.nextcloud[kind].again) && current { self.nextcloud_sync(kind) }
    }

    /// Syncs the Shelf with its Nextcloud folder in the background, as `nextcloud_sync` does a kind.
    pub fn nextcloud_shelf_sync(&mut self) {
        if !self.cfg.nextcloud.enabled() { self.shelf.sync.error = None; return }
        let (Some(me), Some(dir)) = (self.me.clone(), self.store.dir()) else { return };
        if self.shelf.sync.running { self.shelf.sync.again = true; return }
        self.shelf.sync.running = true;
        let account = self.cfg.nextcloud.account();
        let nc = self.cfg.nextcloud.clone();
        let folder = nc.shelf_folder();
        // Another account's folder says nothing of this one's: nothing counts as removed.
        let base: Vec<String> = if nc.synced_with == account { nc.shelf_synced.iter().filter_map(|e| in_folder(e, &folder)).map(str::to_string).collect() } else { Vec::new() };
        let files = self.cfg.shelf.clone();
        me.spawn(move || sync_shelf(&nc, &files, &base, &dir.join("Shelf")), move |s, out| s.nextcloud_shelf_synced(&account, &folder, out));
    }

    /// Files just put on the Shelf (dropped, renamed, compressed, extracted), then a sync. A name
    /// the sync record knows is a new file now, not one Nextcloud since removed.
    pub fn shelf_new_files(&mut self, files: &[PathBuf]) {
        let names: Vec<_> = files.iter().filter_map(|f| f.file_name().map(|n| n.to_string_lossy().into_owned())).collect();
        self.save(|c| c.nextcloud.shelf_synced.retain(|e| !names.iter().any(|n| e.rsplit('/').next() == Some(n.as_str()))));
        if self.shelf.sync.running { self.shelf.sync.dropped.extend(names) }
        self.nextcloud_shelf_sync();
    }

    fn nextcloud_shelf_synced(&mut self, account: &str, folder: &str, out: ShelfOutcome) {
        self.shelf.sync.running = false;
        let current = self.cfg.nextcloud.account() == account;
        if current { self.shelf.sync.error = out.error }
        let changed = self.shelf.sync.again;
        // Dropped while it ran: new files whose names this run's record would wrongly mark as synced.
        let dropped = std::mem::take(&mut self.shelf.sync.dropped);
        if current && let Some(mut both) = out.both {
            self.save(|c| {
                adopt(c, account);
                if changed {
                    // The Shelf changed while it ran, so a file it found gone may have been dropped
                    // again: it stays on, and the rerun decides.
                    for f in &out.gone {
                        let Some(name) = f.file_name().map(|n| n.to_string_lossy().into_owned()) else { continue };
                        both.push(name);
                    }
                }
                both.retain(|n| !dropped.contains(n));
                if !changed {
                    c.shelf.retain(|f| !out.gone.contains(f));
                }
                for f in out.downloaded { if !c.shelf.contains(&f) { c.shelf.push(f) } }
                // Only this folder's: a stale record of another would make its files look removed on a switch back.
                c.nextcloud.shelf_synced = both.into_iter().map(|n| format!("{folder}/{n}")).collect();
            });
        }
        if std::mem::take(&mut self.shelf.sync.again) && current { self.nextcloud_shelf_sync() }
    }
}
