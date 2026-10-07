//! Which GPU the compositor renders on: a DRM render node
//! (`/dev/dri/renderD*`). A render node needs no DRM master, seat or
//! logind session, only read-write access to the device file, which an
//! unprivileged user service has.

use std::{
    ffi::OsString,
    path::{Path, PathBuf},
};

use crate::Error;

/// The environment variable that picks a render node when the
/// configuration does not, such as `/dev/dri/renderD129` for a second
/// GPU.
pub const RENDER_NODE_VAR: &str = "TM_RENDER_NODE";

const DRI_DIR: &str = "/dev/dri";

/// The render node to use: `configured` if given, else
/// [`RENDER_NODE_VAR`], else the first `renderD*` in `/dev/dri`.
pub fn find_render_node(configured: Option<&Path>) -> Result<PathBuf, Error> {
    if let Some(node) = configured {
        return Ok(node.to_owned());
    }
    if let Some(node) =
        std::env::var_os(RENDER_NODE_VAR).filter(|v| !v.is_empty())
    {
        return Ok(PathBuf::from(node));
    }
    let entries = std::fs::read_dir(DRI_DIR).map_err(|e| {
        Error::NoRenderNode(format!("cannot read {DRI_DIR}: {e}"))
    })?;
    let names = entries.filter_map(|entry| entry.ok().map(|e| e.file_name()));
    first_render_node(names)
        .map(|name| Path::new(DRI_DIR).join(name))
        .ok_or_else(|| {
            Error::NoRenderNode(format!("no renderD* device in {DRI_DIR}"))
        })
}

/// The lowest-numbered render node among a directory's entry names.
/// `renderD128` is the first GPU; card nodes (`card1`) need DRM master
/// and are skipped.
fn first_render_node(
    names: impl IntoIterator<Item = OsString>,
) -> Option<OsString> {
    names
        .into_iter()
        .filter_map(|name| {
            let number = name
                .to_str()?
                .strip_prefix("renderD")?
                .parse::<u32>()
                .ok()?;
            Some((number, name))
        })
        .min_by_key(|(number, _)| *number)
        .map(|(_, name)| name)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(list: &[&str]) -> Vec<OsString> {
        list.iter().map(OsString::from).collect()
    }

    #[test]
    fn the_first_render_node_wins_and_card_nodes_are_skipped() {
        assert_eq!(
            first_render_node(names(&[
                "card1",
                "renderD129",
                "by-path",
                "renderD128"
            ])),
            Some(OsString::from("renderD128"))
        );
    }

    #[test]
    fn render_nodes_are_ordered_by_number_not_by_name() {
        assert_eq!(
            first_render_node(names(&["renderD1000", "renderD999"])),
            Some(OsString::from("renderD999"))
        );
    }

    #[test]
    fn no_render_node_is_none() {
        assert_eq!(first_render_node(names(&["card0", "renderDx"])), None);
    }

    #[test]
    fn a_configured_node_is_used_as_given() {
        let node = Path::new("/dev/dri/renderD200");
        assert_eq!(find_render_node(Some(node)).ok(), Some(node.to_owned()));
    }
}
