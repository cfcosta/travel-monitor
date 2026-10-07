//! The GPU side: GBM and EGL on a render node, a GLES renderer that
//! imports the clients' buffers, and one GBM buffer the output is drawn
//! into. Nothing reads the drawn frames; drawing them is what makes the
//! clients see a real display (their buffers are used and released, and
//! frame callbacks follow a frame).
//!
//! Follows moonshine's headless compositor
//! (moonshine-core/src/session/compositor/mod.rs, BSD-2, © Hans
//! Gaiser), without its video export.

use std::{
    fs::{File, OpenOptions},
    os::unix::fs::MetadataExt,
    path::{Path, PathBuf},
};

use smithay::{
    backend::{
        allocator::{
            Allocator,
            Fourcc,
            Modifier,
            dmabuf::{AsDmabuf, Dmabuf},
            format::FormatSet,
            gbm::{GbmAllocator, GbmBuffer, GbmBufferFlags, GbmDevice},
        },
        egl::{EGLContext, EGLDisplay},
        renderer::{
            Bind,
            ImportDma,
            damage::OutputDamageTracker,
            element::surface::WaylandSurfaceRenderElement,
            gles::{Capability, GlesRenderer},
        },
    },
    desktop::{Space, Window, space::render_output},
    output::Output,
};

use crate::Error;

/// The formats tried for the output buffer, in order. 8-bit RGB is all
/// a browser window needs.
const OUTPUT_FORMATS: [Fourcc; 4] = [
    Fourcc::Argb8888,
    Fourcc::Xrgb8888,
    Fourcc::Abgr8888,
    Fourcc::Xbgr8888,
];

/// The background behind the windows: black, as on an empty screen.
const CLEAR_COLOR: [f32; 4] = [0.0, 0.0, 0.0, 1.0];

pub(crate) struct Gpu {
    pub(crate) node: PathBuf,
    /// The render node's device number, which the dmabuf feedback
    /// names as the main device so clients allocate on the same GPU.
    pub(crate) device: u64,
    pub(crate) renderer: GlesRenderer,
    target: Dmabuf,
    /// Owns the memory behind `target`.
    _buffer: GbmBuffer,
    damage: OutputDamageTracker,
    /// How many frames old `target`'s contents are: 0 before the first
    /// frame or after a failed one, then 1 (one buffer, always reused).
    age: usize,
}

impl Gpu {
    /// Opens `node` and sets up GBM, EGL and the renderer, with an
    /// output buffer the size of `output`'s mode.
    pub(crate) fn open(node: &Path, output: &Output) -> Result<Self, Error> {
        let setup = |what: &str, e: &dyn std::fmt::Display| {
            Error::Gpu(format!("{what} on {}: {e}", node.display()))
        };
        // Read-write: mapping GPU buffers needs it (amdgpu fails with
        // EACCES on a read-only node).
        let file: File =
            OpenOptions::new()
                .read(true)
                .write(true)
                .open(node)
                .map_err(|e| setup("cannot open the render node", &e))?;
        let device = file
            .metadata()
            .map_err(|e| setup("cannot stat the render node", &e))?
            .rdev();
        // One GBM device allocates the output buffer, another (on its
        // own file handle) backs the EGL display, as in moonshine.
        let egl_file = file
            .try_clone()
            .map_err(|e| setup("cannot reopen the render node", &e))?;
        let gbm = GbmDevice::new(file)
            .map_err(|e| setup("cannot create a GBM device", &e))?;
        let egl_gbm = GbmDevice::new(egl_file)
            .map_err(|e| setup("cannot create a GBM device", &e))?;
        #[allow(unsafe_code)]
        // SAFETY: the GBM device is a fresh handle on the render node
        // that nothing else uses as an EGL display.
        let display = unsafe { EGLDisplay::new(egl_gbm) }
            .map_err(|e| setup("cannot create an EGL display", &e))?;
        let context = EGLContext::new(&display)
            .map_err(|e| setup("cannot create an EGL context", &e))?;
        let renderer = new_renderer(context)
            .map_err(|e| setup("cannot create a GLES renderer", &e))?;

        let (fourcc, modifiers) =
            output_format(renderer.egl_context().dmabuf_render_formats())
                .ok_or_else(|| {
                    Error::Gpu(format!(
                        "{} renders to none of {OUTPUT_FORMATS:?}",
                        node.display()
                    ))
                })?;
        let size = output
            .current_mode()
            .ok_or_else(|| Error::Gpu("the output has no mode".into()))?
            .size;
        let mut allocator = GbmAllocator::new(gbm, GbmBufferFlags::RENDERING);
        let buffer = allocator
            .create_buffer(
                u32::try_from(size.w).unwrap_or(1),
                u32::try_from(size.h).unwrap_or(1),
                fourcc,
                &modifiers,
            )
            .map_err(|e| setup("cannot allocate the output buffer", &e))?;
        let target = buffer
            .export()
            .map_err(|e| setup("cannot export the output buffer", &e))?;
        tracing::debug!(
            node = %node.display(),
            ?fourcc,
            modifiers = modifiers.len(),
            "output buffer allocated"
        );
        Ok(Self {
            node: node.to_owned(),
            device,
            renderer,
            target,
            _buffer: buffer,
            damage: OutputDamageTracker::from_output(output),
            age: 0,
        })
    }

    /// The formats clients may hand over as dmabufs.
    pub(crate) fn client_formats(&self) -> FormatSet {
        self.renderer.dmabuf_formats()
    }

    /// Draws `space` as `output` shows it. Only what changed since the
    /// last frame is redrawn.
    pub(crate) fn draw(
        &mut self,
        output: &Output,
        space: &Space<Window>,
    ) -> Result<(), Error> {
        let mut framebuffer =
            self.renderer.bind(&mut self.target).map_err(|e| {
                Error::Gpu(format!("cannot bind the output buffer: {e}"))
            })?;
        let drawn = render_output::<
            _,
            WaylandSurfaceRenderElement<GlesRenderer>,
            _,
            _,
        >(
            output,
            &mut self.renderer,
            &mut framebuffer,
            1.0,
            self.age,
            [space],
            &[],
            &mut self.damage,
            CLEAR_COLOR,
        );
        match drawn {
            Ok(_) => {
                self.age = 1;
                Ok(())
            }
            Err(e) => {
                self.age = 0;
                Err(Error::Gpu(format!("cannot draw the output: {e:?}")))
            }
        }
    }
}

/// A GLES renderer with every capability but per-texture fences: the
/// output is never read back, and dmabuf implicit sync already orders
/// the clients' reuse of their buffers (as moonshine does).
fn new_renderer(
    context: EGLContext,
) -> Result<GlesRenderer, smithay::backend::renderer::gles::GlesError> {
    #[allow(unsafe_code)]
    // SAFETY: the context is new and only this thread uses it.
    let capabilities =
        unsafe { GlesRenderer::supported_capabilities(&context) }?
            .into_iter()
            .filter(|c| *c != Capability::Fencing);
    #[allow(unsafe_code)]
    // SAFETY: as above; the renderer takes the context over.
    unsafe {
        GlesRenderer::with_capabilities(context, capabilities)
    }
}

/// The first of [`OUTPUT_FORMATS`] the GPU renders to, with all its
/// modifiers.
fn output_format(formats: &FormatSet) -> Option<(Fourcc, Vec<Modifier>)> {
    OUTPUT_FORMATS.iter().find_map(|&fourcc| {
        let modifiers: Vec<Modifier> = formats
            .iter()
            .filter(|f| f.code == fourcc)
            .map(|f| f.modifier)
            .collect();
        (!modifiers.is_empty()).then_some((fourcc, modifiers))
    })
}

#[cfg(test)]
mod tests {
    use smithay::backend::allocator::Format;

    use super::*;

    #[test]
    fn the_output_prefers_argb_and_keeps_all_its_modifiers() {
        let formats: FormatSet = [
            Format {
                code: Fourcc::Xbgr8888,
                modifier: Modifier::Linear,
            },
            Format {
                code: Fourcc::Argb8888,
                modifier: Modifier::Linear,
            },
            Format {
                code: Fourcc::Argb8888,
                modifier: Modifier::Invalid,
            },
        ]
        .into_iter()
        .collect();
        let (fourcc, modifiers) = output_format(&formats).expect("a format");
        assert_eq!(fourcc, Fourcc::Argb8888);
        assert_eq!(modifiers.len(), 2);
    }

    #[test]
    fn no_rgb_format_is_none() {
        let formats: FormatSet = [Format {
            code: Fourcc::Nv12,
            modifier: Modifier::Linear,
        }]
        .into_iter()
        .collect();
        assert_eq!(output_format(&formats), None);
    }
}
