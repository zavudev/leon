//! The Den in 2.5D: the room as an isometric office, drawn by the GPU in
//! the colours of the host's theme.
//!
//! It is another picture of the same den. The simulation, the feed, the
//! truth card and the roster do not know which picture is shown: this
//! module takes the same [`crate::Den`] and the same [`crate::Frame`] as
//! [`crate::paint`] and gives a picture of the room, and the pixel art stays
//! as the picture of a computer that cannot draw this one.
//!
//! Everything but the GPU is plain arithmetic, and is tested:
//!
//! * [`theme`]: the colours of the room, all from the host's tokens;
//! * [`mesh`]: boxes and faceted balls as triangles and hairlines;
//! * [`camera`]: where a point of the room is in the picture, and back;
//! * [`pieces`]: every piece of the catalogue as a few boxes;
//! * [`lion`]: the lions, their traits and their poses;
//! * [`room`]: a layout and a frame as one mesh, the editor's marks in it,
//!   and what the pointer finds in the room;
//! * [`overlay`]: the name plates, the bubbles and the pointer's boxes,
//!   which the view draws over the picture in the host's font.
//!
//! The GPU itself (`gpu`, behind the `den3d` feature) only turns a mesh
//! into pixels. [`Renderer`] is what the view holds: with the feature off,
//! or on a computer with no adapter, it cannot be started, and the view
//! shows the pixel art.

pub mod camera;
pub mod lion;
pub mod mesh;
pub mod overlay;
pub mod pieces;
pub mod room;
pub mod theme;

#[cfg(feature = "den3d")]
pub mod gpu;

pub use camera::Camera;
pub use mesh::Mesh;
pub use theme::Theme;

/// What draws a mesh: the GPU, when there is one.
pub struct Renderer {
    #[cfg(feature = "den3d")]
    gpu: gpu::Gpu,
}

impl Renderer {
    /// Whether this build can draw the room in 2.5D at all.
    pub const BUILT: bool = cfg!(feature = "den3d");

    /// Opens the GPU. `Err` says why the room cannot be drawn in 2.5D here.
    pub fn start() -> Result<Renderer, String> {
        #[cfg(feature = "den3d")]
        {
            gpu::Gpu::new().map(|gpu| Renderer { gpu })
        }
        #[cfg(not(feature = "den3d"))]
        {
            Err("this build has no 2.5D renderer".to_owned())
        }
    }

    /// What the adapter calls itself.
    pub fn adapter(&self) -> String {
        #[cfg(feature = "den3d")]
        {
            self.gpu.adapter.clone()
        }
        #[cfg(not(feature = "den3d"))]
        {
            String::new()
        }
    }

    /// The picture of a mesh, `width * height * 4` bytes with blue first.
    /// `Err` when the GPU failed: the caller shows the pixel art instead.
    pub fn draw(
        &mut self,
        mesh: &Mesh,
        camera: &Camera,
        theme: &Theme,
        size: (u32, u32),
    ) -> Result<Vec<u8>, String> {
        #[cfg(feature = "den3d")]
        {
            self.gpu
                .draw(mesh, camera, theme.background, theme.light, size)
        }
        #[cfg(not(feature = "den3d"))]
        {
            let _ = (mesh, camera, theme, size);
            Err("this build has no 2.5D renderer".to_owned())
        }
    }

    /// A picture of a room with nobody in it, as large as it fits in
    /// `size`: what a host shows of a den to choose it by. The bytes are as
    /// [`Self::draw`] gives them.
    pub fn room_picture(
        &mut self,
        layout: &crate::layout::DenLayout,
        theme: &Theme,
        size: (u32, u32),
    ) -> Result<Vec<u8>, String> {
        let mut mesh = Mesh::new(theme.edge, true);
        room::furnish(&mut mesh, theme, layout, |_| pieces::Live::default());
        let margin = (size.0.min(size.1) as f32 * 0.03).max(1.);
        let camera = Camera::fit(
            layout.cols,
            layout.rows,
            size.0 as f32,
            size.1 as f32,
            margin,
        );
        self.draw(&mesh, &camera, theme, size)
    }

    /// A picture of one piece of the catalogue, turned so many times, as
    /// large as it fits in `size`: what a host shows of it in a list.
    /// `Err` for an id that is no piece, too.
    pub fn piece_picture(
        &mut self,
        id: &str,
        turn: u8,
        theme: &Theme,
        size: (u32, u32),
    ) -> Result<Vec<u8>, String> {
        let (mesh, low, high) =
            pieces::alone(theme, id, turn).ok_or_else(|| format!("{id} is no piece"))?;
        let margin = (size.0.min(size.1) as f32 * 0.08).max(1.);
        let camera = Camera::around(low, high, size.0 as f32, size.1 as f32, margin);
        self.draw(&mesh, &camera, theme, size)
    }
}

#[cfg(test)]
mod tests;
