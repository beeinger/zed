//! Persist dirty buffers on the daemon so a client close is not a data loss.

use std::path::PathBuf;

use gpui::{App, Context, Entity, TaskExt as _};
use language::{Buffer, BufferEvent, File as _};
use project::{buffer_store::BufferStoreEvent, worktree_store::WorktreeStoreEvent};

use crate::SessionHost;

#[derive(serde::Serialize, serde::Deserialize)]
struct DirtySnapshot {
    abs_path: String,
    text: String,
}

fn snapshot_dir() -> PathBuf {
    paths::remote_server_state_dir().join("dirty_buffers")
}

fn snapshot_path(abs_path: &str) -> PathBuf {
    snapshot_dir().join(format!("{:016x}.json", fnv1a64(abs_path.as_bytes())))
}

fn fnv1a64(bytes: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf29ce484222325;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}

impl SessionHost {
    pub(crate) fn watch_buffers(&mut self, cx: &mut Context<Self>) {
        let buffer_store = self.project.read(cx).buffer_store().clone();
        let worktree_store = self.project.read(cx).worktree_store().clone();
        self.buffer_subscriptions
            .push(cx.subscribe(&buffer_store, |this, _store, event, cx| {
                if let BufferStoreEvent::BufferAdded(buffer) = event {
                    this.watch_one_buffer(buffer.clone(), cx);
                }
            }));
        self.buffer_subscriptions
            .push(cx.subscribe(&worktree_store, |this, _store, event, cx| {
                if matches!(event, WorktreeStoreEvent::WorktreeAdded(_)) {
                    this.restore_dirty_snapshots(cx);
                }
            }));
        let buffers: Vec<_> = buffer_store.read(cx).buffers().collect();
        for buffer in buffers {
            self.watch_one_buffer(buffer, cx);
        }
        self.restore_dirty_snapshots(cx);
    }

    fn watch_one_buffer(&mut self, buffer: Entity<Buffer>, cx: &mut Context<Self>) {
        self.buffer_subscriptions
            .push(cx.subscribe(&buffer, |this, buffer, event, cx| match event {
                BufferEvent::DirtyChanged | BufferEvent::Saved => {
                    this.persist_buffer(&buffer, cx);
                }
                _ => {}
            }));
        self.persist_buffer(&buffer, cx);
    }

    fn persist_buffer(&mut self, buffer: &Entity<Buffer>, cx: &App) {
        let snapshot = buffer.read(cx);
        let Some(abs_path) = snapshot
            .file()
            .and_then(|file| file.as_local().map(|file| file.abs_path(cx)))
        else {
            return;
        };
        persist_snapshot_or_remove(
            &abs_path.to_string_lossy(),
            snapshot.is_dirty(),
            snapshot.text(),
        );
    }

    fn restore_dirty_snapshots(&mut self, cx: &mut Context<Self>) {
        let dir = snapshot_dir();
        let Ok(entries) = std::fs::read_dir(&dir) else {
            return;
        };
        let project = self.project.clone();
        let mut snapshots = Vec::new();
        for entry in entries.flatten() {
            let Ok(bytes) = std::fs::read(entry.path()) else {
                continue;
            };
            let Ok(snapshot) = serde_json::from_slice::<DirtySnapshot>(&bytes) else {
                continue;
            };
            snapshots.push(snapshot);
        }
        if snapshots.is_empty() {
            return;
        }
        cx.spawn(async move |_, cx| {
            for snapshot in snapshots {
                let path = PathBuf::from(&snapshot.abs_path);
                let project_path = project.update(cx, |project, cx| {
                    project.find_project_path(&path, cx)
                });
                let Some(project_path) = project_path else {
                    continue;
                };
                let open = project.update(cx, |project, cx| project.open_buffer(project_path, cx));
                if let Ok(buffer) = open.await {
                    buffer.update(cx, |buffer, cx| {
                        if buffer.is_dirty() {
                            return;
                        }
                        if buffer.text() != snapshot.text {
                            buffer.set_text(snapshot.text.clone(), cx);
                        }
                    });
                }
            }
            anyhow::Ok(())
        })
        .detach_and_log_err(cx);
    }
}

fn persist_snapshot_or_remove(abs_path: &str, is_dirty: bool, text: String) {
    let path = snapshot_path(abs_path);
    if !is_dirty {
        if let Err(error) = std::fs::remove_file(&path)
            && error.kind() != std::io::ErrorKind::NotFound
        {
            log::warn!("remove dirty-buffer snapshot {}: {error}", path.display());
        }
        return;
    }
    if let Some(parent) = path.parent()
        && let Err(error) = std::fs::create_dir_all(parent)
    {
        log::warn!("create dirty-buffer snapshot dir: {error:#}");
        return;
    }
    match serde_json::to_vec(&DirtySnapshot {
        abs_path: abs_path.to_string(),
        text,
    }) {
        Ok(encoded) => {
            if let Err(error) = std::fs::write(&path, encoded) {
                log::warn!("write dirty-buffer snapshot {}: {error}", path.display());
            }
        }
        Err(error) => log::warn!("serialize dirty-buffer snapshot: {error:#}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn path_hash_is_stable() {
        assert_eq!(
            fnv1a64(b"/tmp/app/src/main.rs"),
            fnv1a64(b"/tmp/app/src/main.rs")
        );
        assert_ne!(fnv1a64(b"/tmp/a"), fnv1a64(b"/tmp/b"));
    }
}
