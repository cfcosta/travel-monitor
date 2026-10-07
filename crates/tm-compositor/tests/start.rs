//! The compositor on the machine's real GPU. Skipped (and says so) when
//! there is no render node or no `$XDG_RUNTIME_DIR`, as in the Nix
//! build sandbox. Chromium on it is tested in tm-browser
//! (tests/offscreen.rs).

use std::{os::unix::net::UnixStream, path::PathBuf};

use tm_compositor::{Compositor, CompositorConfig, find_render_node};

fn runtime_dir() -> Option<PathBuf> {
    let Some(dir) = std::env::var_os("XDG_RUNTIME_DIR") else {
        eprintln!("skipping: XDG_RUNTIME_DIR is not set");
        return None;
    };
    if find_render_node(None).is_err() {
        eprintln!("skipping: no GPU render node");
        return None;
    }
    Some(PathBuf::from(dir))
}

#[test]
fn the_socket_accepts_clients_until_shutdown() {
    let Some(dir) = runtime_dir() else {
        return;
    };
    let compositor =
        Compositor::start(CompositorConfig::default()).expect("starts");
    let socket = dir.join(compositor.wayland_display());
    assert!(
        compositor.wayland_display().starts_with("wayland-"),
        "{}",
        compositor.wayland_display()
    );
    UnixStream::connect(&socket).expect("the socket accepts a client");
    compositor.shutdown();
    assert!(!socket.exists(), "{} is left behind", socket.display());
}

#[test]
fn a_missing_render_node_is_an_error() {
    let config = CompositorConfig {
        render_node: Some("/dev/dri/renderD9999".into()),
        ..CompositorConfig::default()
    };
    let error = Compositor::start(config).err().expect("fails");
    assert!(error.to_string().contains("renderD9999"), "{error}");
}

#[test]
fn compositors_get_their_own_sockets() {
    if runtime_dir().is_none() {
        return;
    }
    let a = Compositor::start(CompositorConfig::default()).expect("starts");
    let b = Compositor::start(CompositorConfig::default()).expect("starts");
    assert_ne!(a.wayland_display(), b.wayland_display());
}
