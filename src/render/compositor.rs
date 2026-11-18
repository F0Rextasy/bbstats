//! Windows.UI.Composition host backdrop (Win11 Acrylic) + Direct2D drawing surface.

use crate::platform::dpi::dpi_scale;
use crate::render::device::RenderDevice;
use windows::core::Interface;
use windows::Foundation::Numerics::{Vector2, Vector3};
use windows::Foundation::Size;
use windows::Graphics::DirectX::{DirectXAlphaMode, DirectXPixelFormat};
use windows::System::DispatcherQueueController;
use windows::Win32::Foundation::{HWND, POINT, SIZE};
use windows::Win32::Graphics::Direct2D::{
    ID2D1DeviceContext, D2D1_DEVICE_CONTEXT_OPTIONS_NONE, D2D1_TEXT_ANTIALIAS_MODE_GRAYSCALE,
};
use windows::Win32::System::WinRT::Composition::{
    ICompositionDrawingSurfaceInterop, ICompositorDesktopInterop, ICompositorInterop,
};
use windows::Win32::System::WinRT::{
    CreateDispatcherQueueController, DispatcherQueueOptions, DQTAT_COM_NONE,
    DQTYPE_THREAD_CURRENT,
};
use windows::UI::Composition::Desktop::DesktopWindowTarget;
use windows::UI::Composition::{
    CompositionBrush, CompositionDrawingSurface, CompositionGeometricClip,
    CompositionGraphicsDevice, CompositionRoundedRectangleGeometry, CompositionStretch,
    CompositionSurfaceBrush, Compositor, ContainerVisual, SpriteVisual,
};
use windows::UI::ViewManagement::UISettings;

pub struct CompositionHost {
    _dq_controller: DispatcherQueueController,
    compositor: Compositor,
    _target: DesktopWindowTarget,
    _root: ContainerVisual,
    backdrop_visual: SpriteVisual,
    backdrop_clip_geom: CompositionRoundedRectangleGeometry,
    _backdrop_clip: CompositionGeometricClip,
    host_backdrop_brush: Option<CompositionBrush>,
    backdrop_attempted: bool,
    fallback_brush: CompositionBrush,
    ui_visual: SpriteVisual,
    graphics_device: CompositionGraphicsDevice,
    surface: CompositionDrawingSurface,
    surface_brush: CompositionSurfaceBrush,
    surface_size_px: (i32, i32),
    ui_settings: UISettings,
    acrylic_active: bool,
}

impl CompositionHost {
    pub fn new(hwnd: HWND, device: &RenderDevice, width_px: i32, height_px: i32) -> windows::core::Result<Self> {
        let dq_controller = unsafe {
            CreateDispatcherQueueController(DispatcherQueueOptions {
                dwSize: std::mem::size_of::<DispatcherQueueOptions>() as u32,
                threadType: DQTYPE_THREAD_CURRENT,
                apartmentType: DQTAT_COM_NONE,
            })?
        };

        let compositor = Compositor::new()?;
        let desktop_interop: ICompositorDesktopInterop = compositor.cast()?;
        let target = unsafe { desktop_interop.CreateDesktopWindowTarget(hwnd, true)? };

        let root = compositor.CreateContainerVisual()?;
        target.SetRoot(&root)?;

        let backdrop_visual = compositor.CreateSpriteVisual()?;
        let backdrop_clip_geom = compositor.CreateRoundedRectangleGeometry()?;
        let backdrop_clip = compositor.CreateGeometricClip()?;
        backdrop_clip.SetGeometry(&backdrop_clip_geom)?;
        backdrop_visual.SetClip(&backdrop_clip)?;

        let fallback_brush: CompositionBrush = compositor
            .CreateColorBrushWithColor(windows::UI::Color {
                A: 0,
                R: 11,
                G: 13,
                B: 17,
            })?
            .cast()?;
        backdrop_visual.SetBrush(&fallback_brush)?;

        let ui_settings = UISettings::new()?;

        let compositor_interop: ICompositorInterop = compositor.cast()?;
        let graphics_device = unsafe { compositor_interop.CreateGraphicsDevice(&device.d2d_device)? };

        let w = width_px.max(1);
        let h = height_px.max(1);
        let surface = graphics_device.CreateDrawingSurface(
            Size {
                Width: w as f32,
                Height: h as f32,
            },
            DirectXPixelFormat::B8G8R8A8UIntNormalized,
            DirectXAlphaMode::Premultiplied,
        )?;
        let surface_brush = compositor.CreateSurfaceBrushWithSurface(&surface)?;
        surface_brush.SetStretch(CompositionStretch::None)?;
        surface_brush.SetHorizontalAlignmentRatio(0.0)?;
        surface_brush.SetVerticalAlignmentRatio(0.0)?;

        let ui_visual = compositor.CreateSpriteVisual()?;
        ui_visual.SetBrush(&surface_brush)?;

        let children = root.Children()?;
        children.InsertAtTop(&backdrop_visual)?;
        children.InsertAtTop(&ui_visual)?;

        Ok(Self {
            _dq_controller: dq_controller,
            compositor,
            _target: target,
            _root: root,
            backdrop_visual,
            backdrop_clip_geom,
            _backdrop_clip: backdrop_clip,
            host_backdrop_brush: None,
            backdrop_attempted: false,
            fallback_brush,
            ui_visual,
            graphics_device,
            surface,
            surface_brush,
            surface_size_px: (w, h),
            ui_settings,
            acrylic_active: false,
        })
    }

    pub fn is_acrylic_effective(&self, user_enabled: bool) -> bool {
        user_enabled
            && self.host_backdrop_brush.is_some()
            && self.ui_settings.AdvancedEffectsEnabled().unwrap_or(true)
    }

    pub fn update_backdrop(&mut self, user_enabled: bool) {
        if user_enabled && !self.backdrop_attempted {
            self.backdrop_attempted = true;
            self.host_backdrop_brush = self
                .compositor
                .CreateHostBackdropBrush()
                .ok()
                .and_then(|b| b.cast::<CompositionBrush>().ok());
        }
        let want = self.is_acrylic_effective(user_enabled);
        if want != self.acrylic_active {
            self.acrylic_active = want;
            if want {
                if let Some(ref b) = self.host_backdrop_brush {
                    let _ = self.backdrop_visual.SetBrush(b);
                }
            } else {
                let _ = self.backdrop_visual.SetBrush(&self.fallback_brush);
            }
        }
    }

    /// Updates envelope surface size (only when changed), backdrop offset/clip, and rounded-rectangle corner radius.
    #[allow(clippy::too_many_arguments)]
    pub fn set_geometry(
        &mut self,
        surface_w_px: i32,
        surface_h_px: i32,
        island_off_x_px: f32,
        island_off_y_px: f32,
        island_w_px: f32,
        island_h_px: f32,
        corner_radius_px: f32,
    ) -> windows::core::Result<()> {
        let sw = surface_w_px.max(1);
        let sh = surface_h_px.max(1);
        let iw = island_w_px.max(1.0);
        let ih = island_h_px.max(1.0);
        let surf_size = Vector2 {
            X: sw as f32,
            Y: sh as f32,
        };
        let island_size = Vector2 { X: iw, Y: ih };

        self.backdrop_visual.SetOffset(Vector3 {
            X: island_off_x_px,
            Y: island_off_y_px,
            Z: 0.0,
        })?;
        self.backdrop_visual.SetSize(island_size)?;
        self.backdrop_clip_geom.SetSize(island_size)?;
        self.backdrop_clip_geom.SetCornerRadius(Vector2 {
            X: corner_radius_px,
            Y: corner_radius_px,
        })?;
        self.ui_visual.SetSize(surf_size)?;

        if (sw, sh) != self.surface_size_px {
            let interop: ICompositionDrawingSurfaceInterop = self.surface.cast()?;
            unsafe {
                interop.Resize(SIZE { cx: sw, cy: sh })?;
            }
            self.surface_size_px = (sw, sh);
        }
        Ok(())
    }

    /// Begins a Direct2D draw pass into the composition surface with island-local DIP transform & grayscale AA.
    pub fn begin_draw(
        &self,
        dpi: u32,
        island_off_x_dip: f32,
        island_off_y_dip: f32,
    ) -> windows::core::Result<ID2D1DeviceContext> {
        let interop: ICompositionDrawingSurfaceInterop = self.surface.cast()?;
        let mut offset = POINT::default();
        let dc: ID2D1DeviceContext = unsafe { interop.BeginDraw(None, &mut offset)? };
        unsafe {
            let scale = dpi_scale(dpi);
            dc.SetDpi(dpi as f32, dpi as f32);
            dc.SetTextAntialiasMode(D2D1_TEXT_ANTIALIAS_MODE_GRAYSCALE);
            let _ = D2D1_DEVICE_CONTEXT_OPTIONS_NONE;
            dc.SetTransform(&windows_numerics_matrix(
                offset.x as f32 / scale + island_off_x_dip,
                offset.y as f32 / scale + island_off_y_dip,
            ));
        }
        Ok(dc)
    }

    pub fn end_draw(&self) -> windows::core::Result<()> {
        let interop: ICompositionDrawingSurfaceInterop = self.surface.cast()?;
        unsafe { interop.EndDraw() }
    }

    /// Rebuilds the graphics device and drawing surface after GPU device loss.
    pub fn recreate_surface(
        &mut self,
        device: &RenderDevice,
        width_px: i32,
        height_px: i32,
    ) -> windows::core::Result<()> {
        let compositor_interop: ICompositorInterop = self.compositor.cast()?;
        let graphics_device = unsafe { compositor_interop.CreateGraphicsDevice(&device.d2d_device)? };
        let w = width_px.max(1);
        let h = height_px.max(1);
        let surface = graphics_device.CreateDrawingSurface(
            Size {
                Width: w as f32,
                Height: h as f32,
            },
            DirectXPixelFormat::B8G8R8A8UIntNormalized,
            DirectXAlphaMode::Premultiplied,
        )?;
        self.surface_brush.SetSurface(&surface)?;
        self.graphics_device = graphics_device;
        self.surface = surface;
        self.surface_size_px = (w, h);
        Ok(())
    }
}

#[inline]
fn windows_numerics_matrix(tx: f32, ty: f32) -> windows::Foundation::Numerics::Matrix3x2 {
    windows::Foundation::Numerics::Matrix3x2 {
        M11: 1.0,
        M12: 0.0,
        M21: 0.0,
        M22: 1.0,
        M31: tx,
        M32: ty,
    }
}
