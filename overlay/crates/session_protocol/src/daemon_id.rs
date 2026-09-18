//! Short, stable daemon socket identifiers.
//!
//! `remote_server` names unix sockets as
//! `{data}/server_state/{identifier}/stdout.sock`. The identifier plus that
//! prefix must stay under ~100 bytes (sun_path). Keep the body at 13 chars so
//! a release-channel prefix such as `nightly-` still fits.
//!
//! FNV-1a 64 is used instead of a cryptographic hash so `session_protocol`
//! stays dependency-free. Collision risk is acceptable for local socket names.

/// `s` plus 12 hex digits. Release-channel prefixes are added by the caller.
pub const DAEMON_ID_BODY_LEN: usize = 13;

const FNV_OFFSET: u64 = 0xcbf29ce484222325;
const FNV_PRIME: u64 = 0x100000001b3;

fn fnv1a64(parts: &[&str]) -> u64 {
    let mut hash = FNV_OFFSET;
    for part in parts {
        for byte in part.as_bytes() {
            hash ^= u64::from(*byte);
            hash = hash.wrapping_mul(FNV_PRIME);
        }
        // Domain separator so ("ab", "c") and ("a", "bc") differ.
        hash ^= 0xff;
        hash = hash.wrapping_mul(FNV_PRIME);
    }
    hash
}

fn normalize_project_root(root: &str) -> &str {
    if root.len() <= 1 {
        return root;
    }
    root.trim_end_matches(['/', '\\'])
}

/// Socket-name body for a daemon keyed by transport, host, and project root.
///
/// The result is `s` followed by 12 lowercase hex digits (48 bits of FNV-1a).
/// It does not include a release-channel prefix.
pub fn daemon_socket_id(transport: &str, host: &str, project_root: &str) -> String {
    let project_root = normalize_project_root(project_root);
    let hash = fnv1a64(&[transport, host, project_root]);
    format!("s{:012x}", hash & 0x0000_ffff_ffff_ffff)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn body_length_is_stable() {
        let identifier = daemon_socket_id("ssh", "example.com", "/home/user/proj");
        assert_eq!(identifier.len(), DAEMON_ID_BODY_LEN);
        assert!(identifier.starts_with('s'));
        assert!(
            identifier[1..]
                .chars()
                .all(|character| character.is_ascii_hexdigit())
        );
    }

    #[test]
    fn same_inputs_are_stable() {
        let left = daemon_socket_id("ssh", "host", "/tmp/app");
        let right = daemon_socket_id("ssh", "host", "/tmp/app");
        assert_eq!(left, right);
    }

    #[test]
    fn trailing_slash_does_not_change_id() {
        let without = daemon_socket_id("ssh", "host", "/tmp/app");
        let with = daemon_socket_id("ssh", "host", "/tmp/app/");
        assert_eq!(without, with);
    }

    #[test]
    fn different_roots_differ() {
        let one = daemon_socket_id("ssh", "host", "/tmp/app");
        let two = daemon_socket_id("ssh", "host", "/tmp/other");
        assert_ne!(one, two);
    }

    #[test]
    fn transport_is_part_of_the_key() {
        let ssh = daemon_socket_id("ssh", "localhost", "/tmp/app");
        let local = daemon_socket_id("local", "localhost", "/tmp/app");
        assert_ne!(ssh, local);
    }

    #[test]
    fn concatenation_is_not_ambiguous() {
        let left = daemon_socket_id("ab", "c", "root");
        let right = daemon_socket_id("a", "bc", "root");
        assert_ne!(left, right);
    }
}
