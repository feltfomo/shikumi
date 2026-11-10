//! where a private store keeps its state: its database, its roots, its collection lock and, while
//! a collection runs, the socket a copy hands its temporary root to. when and why the state moves
//! out of the store, how long a moved directory lives and what removes it, and what happens when
//! the temporary directory is cleaned, are in docs/explanation/architecture.md, under the context
//! cache.
//!
//! the move is for Lix 2.95, which connects to a socket path longer than a socket address holds
//! through a helper whose failure isn't the error its collection-socket retry catches (lix 2.95.4
//! libutil/unix-domain-socket.cc:67-73 against libstore/gc.cc:164-174); Lix 2.96 reports it as that
//! error (lix 2.96.0 unix-domain-socket.cc:93-95). the moved state is a real directory, not a link
//! to the store's: a collection's reading of the roots is only defined for the state directory
//! itself. a moved directory and its store each name the other, so whichever outlives the other
//! can be found and removed.

use std::ffi::CString;
use std::io;
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

/// the state directory under a store's root, where Nix keeps it.
pub(super) const STATE: &str = "nix/var/nix";

/// the socket's path under a state directory, as every implementation makes it (cppnix 2.35.3
/// src/libstore/gc.cc:44, determinate 3.24.2 gc.cc:46, lix 2.95.4 and 2.96.0 lix/libstore/gc.cc:36).
const SOCKET: &str = "gc-socket/socket";

/// the longest socket path every platform's socket address holds, macOS's, without its NUL.
const LONGEST_SOCKET: usize = 103;

/// the file in the store that records where its state lives, when not under it.
const RECORD: &str = "state-dir";

/// the file in a moved state directory that names the store it belongs to.
const BELONGS: &str = "store";

/// what a moved state directory's name starts with, before its maker's process.
const PREFIX: &str = "shikumi";

/// the store at `store`'s state directory: under it, or where its record says, or a new one in
/// `temporary`.
pub(super) fn of(store: &Path, temporary: &Path) -> io::Result<PathBuf> {
    let own = store.join(STATE);
    if own.join(SOCKET).as_os_str().len() <= LONGEST_SOCKET {
        return Ok(own);
    }
    if let Some(recorded) = recorded(store) {
        if this_users_alone(&recorded) {
            return Ok(recorded);
        }
        remove_if_its(store, &recorded);
    }
    let directory = made(store, temporary)?;
    let fresh = store.join(super::owned(&format!(".{RECORD}")));
    std::fs::write(&fresh, directory.as_os_str().as_bytes())?;
    std::fs::rename(&fresh, store.join(RECORD)).inspect_err(|_| {
        let _ = std::fs::remove_file(&fresh);
        let _ = super::remove_tree(&directory);
    })?;
    Ok(directory)
}

/// removes the moved state directory of the store at `store`, which is about to go.
pub(super) fn forget(store: &Path) {
    if let Some(recorded) = recorded(store) {
        remove_if_its(store, &recorded);
    }
}

/// removes every moved state directory in `temporary` its store doesn't record, once the process
/// that made it, and would record it, is gone.
pub(super) fn sweep(temporary: &Path) {
    let Ok(entries) = std::fs::read_dir(temporary) else {
        return;
    };
    for entry in entries.filter_map(Result::ok) {
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            continue;
        };
        let made_by = name
            .strip_prefix(PREFIX)
            .filter(|rest| rest.starts_with('-'))
            .and(super::owner(name));
        let directory = entry.path();
        if made_by.is_none_or(super::alive) || !this_users(&directory) {
            continue;
        }
        let kept = belongs(&directory)
            .and_then(|store| recorded(&store))
            .is_some_and(|recorded| recorded == directory);
        if !kept {
            let _ = super::remove_tree(&directory);
        }
    }
}

/// where the store's record says its state lives.
fn recorded(store: &Path) -> Option<PathBuf> {
    std::fs::read(store.join(RECORD))
        .ok()
        .map(|recorded| PathBuf::from(std::ffi::OsString::from_vec(recorded)))
}

/// the store a moved state directory names.
fn belongs(directory: &Path) -> Option<PathBuf> {
    std::fs::read(directory.join(BELONGS))
        .ok()
        .map(|store| PathBuf::from(std::ffi::OsString::from_vec(store)))
}

/// removes `directory` if it's this user's and names `store` as its own; one that isn't is
/// another's to remove.
fn remove_if_its(store: &Path, directory: &Path) {
    if this_users(directory) && belongs(directory).as_deref() == Some(store) {
        let _ = super::remove_tree(directory);
    }
}

/// whether `directory` is a real directory this user owns.
fn this_users(directory: &Path) -> bool {
    // SAFETY: `geteuid` reads the process's own user and can't fail.
    let user = unsafe { libc::geteuid() };
    std::fs::symlink_metadata(directory)
        .is_ok_and(|metadata| metadata.is_dir() && metadata.uid() == user)
}

/// whether `directory` is a real directory only this user can write.
fn this_users_alone(directory: &Path) -> bool {
    this_users(directory)
        && std::fs::symlink_metadata(directory).is_ok_and(|metadata| metadata.mode() & 0o077 == 0)
}

/// a new directory of this user's alone in `temporary`, named for this process and naming `store`.
fn made(store: &Path, temporary: &Path) -> io::Result<PathBuf> {
    let template = temporary.join(format!("{PREFIX}-{}-XXXXXX", std::process::id()));
    let template = CString::new(template.into_os_string().into_vec())
        .map_err(|_| io::Error::other("the temporary directory's path holds a NUL"))?;
    let mut bytes = template.into_bytes_with_nul();
    // SAFETY: `bytes` is a NUL-terminated template `mkdtemp` writes the name into, in place.
    let made = unsafe { libc::mkdtemp(bytes.as_mut_ptr().cast()) };
    if made.is_null() {
        return Err(io::Error::last_os_error());
    }
    bytes.pop();
    let directory = PathBuf::from(std::ffi::OsString::from_vec(bytes));
    std::fs::write(directory.join(BELONGS), store.as_os_str().as_bytes()).inspect_err(|_| {
        let _ = super::remove_tree(&directory);
    })?;
    Ok(directory)
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::PermissionsExt;

    use super::*;

    fn store(base: &Path, long: usize) -> PathBuf {
        let store = base
            .join("a".repeat(long))
            .join(super::super::STORES)
            .join("0123456789abcdef");
        std::fs::create_dir_all(store.join(STATE)).unwrap();
        store
    }

    // a store whose own state directory puts the socket within every platform's limit keeps its
    // state there.
    #[test]
    fn a_short_store_keeps_its_state_under_it() {
        let base = tempfile::tempdir().unwrap();
        let store = store(base.path(), 1);
        assert_eq!(of(&store, base.path()).unwrap(), store.join(STATE));
    }

    // under a cache directory long enough that the socket's own path needs Lix's helper, the state
    // lives where the socket's path is short, in a real directory, the same one every run.
    #[test]
    fn a_long_store_keeps_its_state_where_the_socket_fits() {
        let base = tempfile::tempdir().unwrap();
        let store = store(base.path(), 120);
        assert!(store.join(STATE).join(SOCKET).as_os_str().len() > LONGEST_SOCKET);
        let state = of(&store, base.path()).unwrap();
        assert!(state.join(SOCKET).as_os_str().len() <= LONGEST_SOCKET);
        assert!(std::fs::symlink_metadata(&state).unwrap().is_dir());
        assert_eq!(of(&store, base.path()).unwrap(), state);
        super::super::remove_tree(&state).unwrap();
    }

    // a relocated state directory lives as long as its store does: the one a record stops naming,
    // the one of a store maintenance forgets, and the one of a store removed while no run of its
    // maker's was left all go, so repeated runs leave only what live stores record.
    #[test]
    fn repeated_runs_leave_only_the_live_state_directories() {
        let base = tempfile::tempdir().unwrap();
        let temporary = tempfile::tempdir().unwrap();
        let named = |name: &str| {
            let store = base
                .path()
                .join("a".repeat(120))
                .join(super::super::STORES)
                .join(name);
            std::fs::create_dir_all(store.join(STATE)).unwrap();
            store
        };
        let replaced = named("replaced");
        for _ in 0..5 {
            let state = of(&replaced, temporary.path()).unwrap();
            std::fs::set_permissions(&state, std::fs::Permissions::from_mode(0o777)).unwrap();
        }
        let live = of(&replaced, temporary.path()).unwrap();

        let forgotten = named("forgotten");
        of(&forgotten, temporary.path()).unwrap();
        forget(&forgotten);
        super::super::remove_tree(&forgotten).unwrap();

        // made by a run that's gone, as the dead process's own name says, then removed by hand
        // with the cache it was in.
        let removed = named("removed");
        let state = of(&removed, temporary.path()).unwrap();
        let orphan = temporary.path().join(format!("{PREFIX}-{}-gone", i32::MAX));
        std::fs::rename(&state, &orphan).unwrap();
        std::fs::write(removed.join(RECORD), orphan.as_os_str().as_bytes()).unwrap();
        super::super::remove_tree(&removed).unwrap();

        sweep(temporary.path());
        let left: Vec<PathBuf> = std::fs::read_dir(temporary.path())
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .collect();
        assert_eq!(left, [live]);
    }

    // a recorded directory another user could write, or one that's gone, isn't used.
    #[test]
    fn a_state_directory_that_isnt_this_users_alone_is_made_again() {
        let base = tempfile::tempdir().unwrap();
        let store = store(base.path(), 120);
        let first = of(&store, base.path()).unwrap();
        std::fs::set_permissions(&first, std::fs::Permissions::from_mode(0o777)).unwrap();
        let second = of(&store, base.path()).unwrap();
        assert_ne!(first, second);
        super::super::remove_tree(&second).unwrap();
        let third = of(&store, base.path()).unwrap();
        assert_ne!(second, third);
        for made in [&first, &third] {
            let _ = super::super::remove_tree(made);
        }
    }
}
