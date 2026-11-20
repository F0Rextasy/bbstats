//! Reusable Direct2D / DirectWrite Obsidian Telemetry drawing primitives.

use crate::render::theme::*;
use std::cell::RefCell;
use std::mem::ManuallyDrop;
use windows::core::Interface;
use windows::Win32::Graphics::Direct2D::Common::{
    D2D1_ALPHA_MODE_PREMULTIPLIED, D2D1_BEZIER_SEGMENT, D2D1_COLOR_F, D2D1_GRADIENT_STOP,
    D2D_POINT_2F, D2D_RECT_F,
};
use windows::Win32::Graphics::Direct2D::{
    ID2D1DeviceContext, ID2D1Factory1, ID2D1Geometry, ID2D1LinearGradientBrush,
    ID2D1RadialGradientBrush, ID2D1SolidColorBrush, D2D1_ANTIALIAS_MODE_PER_PRIMITIVE,
    D2D1_BRUSH_PROPERTIES, D2D1_BUFFER_PRECISION_8BPC_UNORM, D2D1_CAP_STYLE_ROUND,
    D2D1_COLOR_INTERPOLATION_MODE_PREMULTIPLIED, D2D1_COLOR_SPACE_SRGB,
    D2D1_DRAW_TEXT_OPTIONS_CLIP, D2D1_DRAW_TEXT_OPTIONS_ENABLE_COLOR_FONT, D2D1_ELLIPSE,
    D2D1_EXTEND_MODE_CLAMP, D2D1_FEATURE_LEVEL_DEFAULT, D2D1_LAYER_OPTIONS1_NONE,
    D2D1_LAYER_PARAMETERS1, D2D1_LINEAR_GRADIENT_BRUSH_PROPERTIES,
    D2D1_RADIAL_GRADIENT_BRUSH_PROPERTIES, D2D1_ROUNDED_RECT, D2D1_STROKE_STYLE_PROPERTIES1,
};
use windows::Win32::Graphics::DirectWrite::{
    IDWriteFactory, IDWriteTextFormat, IDWriteTextLayout, IDWriteTypography, DWRITE_TEXT_RANGE,
};

pub struct DrawCtx<'a> {
    pub dc: &'a ID2D1DeviceContext,
    pub d2d_factory: &'a ID2D1Factory1,
    pub dw_factory: &'a IDWriteFactory,
    pub tabular: &'a IDWriteTypography,
    brush: ID2D1SolidColorBrush,
    wide_buf: RefCell<Vec<u16>>,
}

impl<'a> DrawCtx<'a> {
    pub fn new(
        dc: &'a ID2D1DeviceContext,
        d2d_factory: &'a ID2D1Factory1,
        dw_factory: &'a IDWriteFactory,
        tabular: &'a IDWriteTypography,
    ) -> windows::core::Result<Self> {
        let brush = unsafe { dc.CreateSolidColorBrush(&BORDER_SUBTLE, None)? };
        Ok(Self {
            dc,
            d2d_factory,
            dw_factory,
            tabular,
            brush,
            wide_buf: RefCell::new(Vec::with_capacity(128)),
        })
    }

    #[inline]
    pub fn solid(&self, color: D2D1_COLOR_F) -> &ID2D1SolidColorBrush {
        unsafe {
            self.brush.SetColor(&color);
        }
        &self.brush
    }

    pub fn with_axis_clip<F>(&self, rect: D2D_RECT_F, f: F) -> windows::core::Result<()>
    where
        F: FnOnce() -> windows::core::Result<()>,
    {
        unsafe {
            self.dc.PushAxisAlignedClip(&rect, D2D1_ANTIALIAS_MODE_PER_PRIMITIVE);
        }
        let res = f();
        unsafe {
            self.dc.PopAxisAlignedClip();
        }
        res
    }

    pub fn fill_rounded(
        &self,
        rect: D2D_RECT_F,
        radius: f32,
        color: D2D1_COLOR_F,
    ) -> windows::core::Result<()> {
        let brush = self.solid(color);
        let rr = D2D1_ROUNDED_RECT {
            rect,
            radiusX: radius,
            radiusY: radius,
        };
        unsafe {
            self.dc.FillRoundedRectangle(&rr, brush);
        }
        Ok(())
    }

    pub fn stroke_rounded(
        &self,
        rect: D2D_RECT_F,
        radius: f32,
        color: D2D1_COLOR_F,
        width: f32,
    ) -> windows::core::Result<()> {
        let brush = self.solid(color);
        let rr = D2D1_ROUNDED_RECT {
            rect,
            radiusX: radius,
            radiusY: radius,
        };
        unsafe {
            self.dc.DrawRoundedRectangle(&rr, brush, width, None);
        }
        Ok(())
    }

    /// Feathered ambient drop shadow drawn outside the pill bounds when envelope padding exists.
    pub fn draw_ambient_shadow(
        &self,
        w: f32,
        h: f32,
        radius: f32,
        strength: f32,
    ) -> windows::core::Result<()> {
        if strength <= 0.02 {
            return Ok(());
        }
        let rings = [
            (2.0f32, 1.0f32, 0.14f32),
            (5.0f32, 2.0f32, 0.09f32),
            (9.0f32, 3.0f32, 0.05f32),
            (13.0f32, 4.0f32, 0.022f32),
        ];
        for (exp, dy, alpha) in rings {
            let r = D2D_RECT_F {
                left: -exp,
                top: -exp + dy,
                right: w + exp,
                bottom: h + exp + dy,
            };
            self.fill_rounded(r, radius + exp * 0.75, rgba(0, 0, 0, alpha * strength))?;
        }
        Ok(())
    }

    /// Clips all inner drawing strictly to an anti-aliased rounded-rectangle layer.
    pub fn with_rounded_clip<F>(
        &self,
        rect: D2D_RECT_F,
        radius: f32,
        f: F,
    ) -> windows::core::Result<()>
    where
        F: FnOnce() -> windows::core::Result<()>,
    {
        unsafe {
            let rr = D2D1_ROUNDED_RECT {
                rect,
                radiusX: radius,
                radiusY: radius,
            };
            let geo = self.d2d_factory.CreateRoundedRectangleGeometry(&rr)?;
            let mask: ID2D1Geometry = geo.cast()?;
            let params = D2D1_LAYER_PARAMETERS1 {
                contentBounds: rect,
                geometricMask: ManuallyDrop::new(Some(mask)),
                maskAntialiasMode: D2D1_ANTIALIAS_MODE_PER_PRIMITIVE,
                maskTransform: identity_matrix(),
                opacity: 1.0,
                opacityBrush: ManuallyDrop::new(None),
                layerOptions: D2D1_LAYER_OPTIONS1_NONE,
            };
            self.dc.PushLayer(&params, None);
            let res = f();
            self.dc.PopLayer();
            let _ = ManuallyDrop::into_inner(params.geometricMask);
            let _ = ManuallyDrop::into_inner(params.opacityBrush);
            res
        }
    }

    /// Executes `f()` under a scaled/translated layer with unified opacity (`opacity` in `0.0..=1.0`).
    pub fn with_transformed_layer<F>(
        &self,
        offset_x: f32,
        offset_y: f32,
        scale_x: f32,
        scale_y: f32,
        opacity: f32,
        f: F,
    ) -> windows::core::Result<()>
    where
        F: FnOnce() -> windows::core::Result<()>,
    {
        unsafe {
            let mut base = identity_matrix();
            self.dc.GetTransform(&mut base);
            let next = windows::Foundation::Numerics::Matrix3x2 {
                M11: scale_x * base.M11,
                M12: 0.0,
                M21: 0.0,
                M22: scale_y * base.M22,
                M31: offset_x * base.M11 + base.M31,
                M32: offset_y * base.M22 + base.M32,
            };
            self.dc.SetTransform(&next);
            let res = if opacity < 0.995 {
                let params = D2D1_LAYER_PARAMETERS1 {
                    contentBounds: D2D_RECT_F {
                        left: -2000.0,
                        top: -2000.0,
                        right: 4000.0,
                        bottom: 4000.0,
                    },
                    geometricMask: ManuallyDrop::new(None),
                    maskAntialiasMode: D2D1_ANTIALIAS_MODE_PER_PRIMITIVE,
                    maskTransform: identity_matrix(),
                    opacity: opacity.clamp(0.0, 1.0),
                    opacityBrush: ManuallyDrop::new(None),
                    layerOptions: D2D1_LAYER_OPTIONS1_NONE,
                };
                self.dc.PushLayer(&params, None);
                let inner = f();
                self.dc.PopLayer();
                let _ = ManuallyDrop::into_inner(params.geometricMask);
                let _ = ManuallyDrop::into_inner(params.opacityBrush);
                inner
            } else {
                f()
            };
            self.dc.SetTransform(&base);
            res
        }
    }

    /// Draws the smoked glass background + 1px dark outer rim + 1px top specular gradient rim.
    /// When `bg_color.a <= 0.04` and no alert tint is present, skips borders for pure transparent HUD.
    pub fn draw_glass_shell(
        &self,
        w: f32,
        h: f32,
        radius: f32,
        bg_color: D2D1_COLOR_F,
        tint_border: Option<D2D1_COLOR_F>,
    ) -> windows::core::Result<()> {
        if bg_color.a <= 0.04 && tint_border.is_none() {
            return Ok(());
        }
        let outer = D2D_RECT_F {
            left: 0.5,
            top: 0.5,
            right: (w - 0.5).max(1.0),
            bottom: (h - 0.5).max(1.0),
        };
        if bg_color.a > 0.01 {
            self.fill_rounded(outer, radius, bg_color)?;
        }

        let rim_scale = if tint_border.is_some() {
            1.0
        } else {
            (bg_color.a / 0.85).clamp(0.18, 1.0)
        };

        // Subtle outer dark rim for contrast against light wallpapers
        self.stroke_rounded(
            outer,
            radius,
            D2D1_COLOR_F {
                a: BORDER_OUTER_DARK.a * rim_scale,
                ..BORDER_OUTER_DARK
            },
            1.0,
        )?;

        // Inner 1px top specular rim (linear gradient top->bottom)
        let inner = D2D_RECT_F {
            left: 1.25,
            top: 1.25,
            right: (w - 1.25).max(2.0),
            bottom: (h - 1.25).max(2.0),
        };
        let mut top_c = tint_border.unwrap_or(BORDER_SPECULAR_TOP);
        top_c.a *= rim_scale;
        let bot_c = tint_border
            .map(|c| D2D1_COLOR_F {
                a: c.a * 0.25 * rim_scale,
                ..c
            })
            .unwrap_or(D2D1_COLOR_F {
                a: BORDER_SPECULAR_BOT.a * rim_scale,
                ..BORDER_SPECULAR_BOT
            });
        let stops = [
            D2D1_GRADIENT_STOP {
                position: 0.0,
                color: top_c,
            },
            D2D1_GRADIENT_STOP {
                position: 0.45,
                color: D2D1_COLOR_F {
                    a: top_c.a * 0.45,
                    ..top_c
                },
            },
            D2D1_GRADIENT_STOP {
                position: 1.0,
                color: bot_c,
            },
        ];
        unsafe {
            let stop_coll = self.dc.CreateGradientStopCollection(
                &stops,
                D2D1_COLOR_SPACE_SRGB,
                D2D1_COLOR_SPACE_SRGB,
                D2D1_BUFFER_PRECISION_8BPC_UNORM,
                D2D1_EXTEND_MODE_CLAMP,
                D2D1_COLOR_INTERPOLATION_MODE_PREMULTIPLIED,
            )?;
            let grad: ID2D1LinearGradientBrush = self.dc.CreateLinearGradientBrush(
                &D2D1_LINEAR_GRADIENT_BRUSH_PROPERTIES {
                    startPoint: D2D_POINT_2F { x: 0.0, y: 0.0 },
                    endPoint: D2D_POINT_2F { x: 0.0, y: h },
                },
                Some(&D2D1_BRUSH_PROPERTIES {
                    opacity: 1.0,
                    transform: identity_matrix(),
                }),
                &stop_coll,
            )?;
            let rr = D2D1_ROUNDED_RECT {
                rect: inner,
                radiusX: (radius - 1.0).max(2.0),
                radiusY: (radius - 1.0).max(2.0),
            };
            self.dc.DrawRoundedRectangle(&rr, &grad, 1.0, None);

            // Subtle inner bottom rim shadow for machined glass bevel depth.
            let bot_line_y = (h - 1.5).max(2.0);
            let bot_pad = (radius * 0.45).max(8.0);
            if w > bot_pad * 2.0 + 4.0 {
                let bbrush = self.solid(rgba(0, 0, 0, 0.24 * rim_scale));
                self.dc.DrawLine(
                    D2D_POINT_2F {
                        x: bot_pad,
                        y: bot_line_y,
                    },
                    D2D_POINT_2F {
                        x: w - bot_pad,
                        y: bot_line_y,
                    },
                    bbrush,
                    1.0,
                    None,
                );
            }
        }
        Ok(())
    }

    fn radial_brush(
        &self,
        center: D2D_POINT_2F,
        rx: f32,
        ry: f32,
        stops: &[D2D1_GRADIENT_STOP],
    ) -> windows::core::Result<ID2D1RadialGradientBrush> {
        unsafe {
            let stop_coll = self.dc.CreateGradientStopCollection(
                stops,
                D2D1_COLOR_SPACE_SRGB,
                D2D1_COLOR_SPACE_SRGB,
                D2D1_BUFFER_PRECISION_8BPC_UNORM,
                D2D1_EXTEND_MODE_CLAMP,
                D2D1_COLOR_INTERPOLATION_MODE_PREMULTIPLIED,
            )?;
            self.dc.CreateRadialGradientBrush(
                &D2D1_RADIAL_GRADIENT_BRUSH_PROPERTIES {
                    center,
                    gradientOriginOffset: D2D_POINT_2F { x: 0.0, y: 0.0 },
                    radiusX: rx.max(1.0),
                    radiusY: ry.max(1.0),
                },
                None,
                &stop_coll,
            )
        }
    }

    /// Subtle top-edge status-tinted radial aura and horizontal light crown inside the glass clip.
    pub fn draw_top_ambient_glow(
        &self,
        w: f32,
        h: f32,
        color: D2D1_COLOR_F,
        intensity: f32,
    ) -> windows::core::Result<()> {
        if intensity <= 0.005 {
            return Ok(());
        }
        let intensity = intensity.clamp(0.0, 1.0);
        let center = D2D_POINT_2F { x: w * 0.5, y: 0.0 };
        let rx = (w * 0.62).max(64.0);
        let ry = (h * 0.42).clamp(16.0, 88.0);
        let aura_stops = [
            D2D1_GRADIENT_STOP {
                position: 0.0,
                color: D2D1_COLOR_F { a: intensity, ..color },
            },
            D2D1_GRADIENT_STOP {
                position: 0.55,
                color: D2D1_COLOR_F {
                    a: intensity * 0.32,
                    ..color
                },
            },
            D2D1_GRADIENT_STOP {
                position: 1.0,
                color: D2D1_COLOR_F { a: 0.0, ..color },
            },
        ];
        let aura = self.radial_brush(center, rx, ry, &aura_stops)?;
        unsafe {
            self.dc.FillEllipse(
                &D2D1_ELLIPSE {
                    point: center,
                    radiusX: rx,
                    radiusY: ry,
                },
                &aura,
            );
        }

        let x1 = (w * 0.16).max(10.0);
        let x2 = (w * 0.84).min(w - 10.0);
        if x2 > x1 + 4.0 {
            let crown_stops = [
                D2D1_GRADIENT_STOP {
                    position: 0.0,
                    color: D2D1_COLOR_F { a: 0.0, ..color },
                },
                D2D1_GRADIENT_STOP {
                    position: 0.5,
                    color: D2D1_COLOR_F {
                        a: (intensity * 2.4).clamp(0.0, 0.58),
                        ..color
                    },
                },
                D2D1_GRADIENT_STOP {
                    position: 1.0,
                    color: D2D1_COLOR_F { a: 0.0, ..color },
                },
            ];
            unsafe {
                let coll = self.dc.CreateGradientStopCollection(
                    &crown_stops,
                    D2D1_COLOR_SPACE_SRGB,
                    D2D1_COLOR_SPACE_SRGB,
                    D2D1_BUFFER_PRECISION_8BPC_UNORM,
                    D2D1_EXTEND_MODE_CLAMP,
                    D2D1_COLOR_INTERPOLATION_MODE_PREMULTIPLIED,
                )?;
                let crown: ID2D1LinearGradientBrush = self.dc.CreateLinearGradientBrush(
                    &D2D1_LINEAR_GRADIENT_BRUSH_PROPERTIES {
                        startPoint: D2D_POINT_2F { x: x1, y: 1.35 },
                        endPoint: D2D_POINT_2F { x: x2, y: 1.35 },
                    },
                    None,
                    &coll,
                )?;
                self.dc.DrawLine(
                    D2D_POINT_2F { x: x1, y: 1.35 },
                    D2D_POINT_2F { x: x2, y: 1.35 },
                    &crown,
                    1.4,
                    None,
                );
            }
        }
        Ok(())
    }

    /// Subtle cursor-tracking glass surface spotlight and top-rim specular reflection.
    pub fn draw_cursor_specular(
        &self,
        w: f32,
        h: f32,
        radius: f32,
        cursor: (f32, f32),
        accent: D2D1_COLOR_F,
    ) -> windows::core::Result<()> {
        let (cx, cy) = cursor;
        if cx < -24.0 || cy < -24.0 || cx > w + 24.0 || cy > h + 24.0 {
            return Ok(());
        }

        let spot_stops = [
            D2D1_GRADIENT_STOP {
                position: 0.0,
                color: rgba(255, 255, 255, 0.055),
            },
            D2D1_GRADIENT_STOP {
                position: 0.5,
                color: D2D1_COLOR_F { a: 0.022, ..accent },
            },
            D2D1_GRADIENT_STOP {
                position: 1.0,
                color: rgba(255, 255, 255, 0.0),
            },
        ];
        let spot = self.radial_brush(D2D_POINT_2F { x: cx, y: cy }, 115.0, 78.0, &spot_stops)?;
        let full = D2D_RECT_F {
            left: 0.0,
            top: 0.0,
            right: w,
            bottom: h,
        };
        unsafe {
            self.dc.FillRectangle(&full, &spot);
        }

        let rim_stops = [
            D2D1_GRADIENT_STOP {
                position: 0.0,
                color: rgba(255, 255, 255, 0.46),
            },
            D2D1_GRADIENT_STOP {
                position: 0.45,
                color: D2D1_COLOR_F { a: 0.20, ..accent },
            },
            D2D1_GRADIENT_STOP {
                position: 1.0,
                color: rgba(255, 255, 255, 0.0),
            },
        ];
        let rim = self.radial_brush(
            D2D_POINT_2F {
                x: cx,
                y: cy.min(56.0),
            },
            128.0,
            72.0,
            &rim_stops,
        )?;
        let inner = D2D_RECT_F {
            left: 1.25,
            top: 1.25,
            right: (w - 1.25).max(2.0),
            bottom: (h - 1.25).max(2.0),
        };
        let rr = D2D1_ROUNDED_RECT {
            rect: inner,
            radiusX: (radius - 1.0).max(2.0),
            radiusY: (radius - 1.0).max(2.0),
        };
        unsafe {
            self.dc.DrawRoundedRectangle(&rr, &rim, 1.25, None);
        }
        Ok(())
    }

    pub fn draw_hairline(&self, x1: f32, y: f32, x2: f32) -> windows::core::Result<()> {
        let brush = self.solid(BORDER_SUBTLE);
        unsafe {
            self.dc.DrawLine(
                D2D_POINT_2F { x: x1, y },
                D2D_POINT_2F { x: x2, y },
                brush,
                1.0,
                None,
            );
        }
        Ok(())
    }

    pub fn draw_vline(&self, x: f32, y1: f32, y2: f32) -> windows::core::Result<()> {
        let brush = self.solid(BORDER_SUBTLE);
        unsafe {
            self.dc.DrawLine(
                D2D_POINT_2F { x, y: y1 },
                D2D_POINT_2F { x, y: y2 },
                brush,
                1.0,
                None,
            );
        }
        Ok(())
    }

    /// Draws a live status dot with optional restrained radial glow.
    pub fn draw_status_dot(
        &self,
        cx: f32,
        cy: f32,
        radius: f32,
        color: D2D1_COLOR_F,
        glow: bool,
    ) -> windows::core::Result<()> {
        if glow {
            let stops = [
                D2D1_GRADIENT_STOP {
                    position: 0.0,
                    color: D2D1_COLOR_F { a: 0.42, ..color },
                },
                D2D1_GRADIENT_STOP {
                    position: 1.0,
                    color: D2D1_COLOR_F { a: 0.0, ..color },
                },
            ];
            unsafe {
                let stop_coll = self.dc.CreateGradientStopCollection(
                    &stops,
                    D2D1_COLOR_SPACE_SRGB,
                    D2D1_COLOR_SPACE_SRGB,
                    D2D1_BUFFER_PRECISION_8BPC_UNORM,
                    D2D1_EXTEND_MODE_CLAMP,
                    D2D1_COLOR_INTERPOLATION_MODE_PREMULTIPLIED,
                )?;
                let glow_r = radius * 2.5;
                let brush: ID2D1RadialGradientBrush = self.dc.CreateRadialGradientBrush(
                    &D2D1_RADIAL_GRADIENT_BRUSH_PROPERTIES {
                        center: D2D_POINT_2F { x: cx, y: cy },
                        gradientOriginOffset: D2D_POINT_2F { x: 0.0, y: 0.0 },
                        radiusX: glow_r,
                        radiusY: glow_r,
                    },
                    None,
                    &stop_coll,
                )?;
                self.dc.FillEllipse(
                    &D2D1_ELLIPSE {
                        point: D2D_POINT_2F { x: cx, y: cy },
                        radiusX: glow_r,
                        radiusY: glow_r,
                    },
                    &brush,
                );
            }
        }
        let dot_brush = self.solid(color);
        unsafe {
            self.dc.FillEllipse(
                &D2D1_ELLIPSE {
                    point: D2D_POINT_2F { x: cx, y: cy },
                    radiusX: radius,
                    radiusY: radius,
                },
                dot_brush,
            );
        }
        Ok(())
    }

    /// Draws text with optional tabular-nums typography feature applied.
    pub fn draw_text(
        &self,
        text: &str,
        fmt: &IDWriteTextFormat,
        rect: D2D_RECT_F,
        color: D2D1_COLOR_F,
        tabular: bool,
    ) -> windows::core::Result<()> {
        let w = (rect.right - rect.left).max(1.0);
        let h = (rect.bottom - rect.top).max(1.0);
        let mut wide = self.wide_buf.borrow_mut();
        wide.clear();
        wide.extend(text.encode_utf16());
        let brush = self.solid(color);
        unsafe {
            let layout: IDWriteTextLayout =
                self.dw_factory.CreateTextLayout(&wide, fmt, w, h)?;
            if tabular && !wide.is_empty() {
                let _ = layout.SetTypography(
                    self.tabular,
                    DWRITE_TEXT_RANGE {
                        startPosition: 0,
                        length: wide.len() as u32,
                    },
                );
            }
            self.dc.DrawTextLayout(
                D2D_POINT_2F {
                    x: rect.left,
                    y: rect.top,
                },
                &layout,
                brush,
                D2D1_DRAW_TEXT_OPTIONS_CLIP | D2D1_DRAW_TEXT_OPTIONS_ENABLE_COLOR_FONT,
            );
        }
        Ok(())
    }

    /// Draws a smooth Catmull-Rom Bezier sparkline + gradient fill + live endpoint beacon.
    pub fn draw_sparkline(
        &self,
        rect: D2D_RECT_F,
        samples: &[f32],
        max_val: f32,
        stroke_color: D2D1_COLOR_F,
    ) -> windows::core::Result<()> {
        if samples.len() < 2 {
            return Ok(());
        }
        let w = (rect.right - rect.left).max(1.0);
        let h = (rect.bottom - rect.top).max(1.0);
        let ceil = max_val.max(1.0);
        let step_x = w / (samples.len() - 1) as f32;
        let pts: Vec<D2D_POINT_2F> = samples
            .iter()
            .enumerate()
            .map(|(i, &val)| D2D_POINT_2F {
                x: rect.left + i as f32 * step_x,
                y: rect.bottom - (val.clamp(0.0, ceil) / ceil) * (h - 6.0) - 3.0,
            })
            .collect();

        unsafe {
            let fill_geo = self.d2d_factory.CreatePathGeometry()?;
            let fill_sink = fill_geo.Open()?;
            let stroke_geo = self.d2d_factory.CreatePathGeometry()?;
            let stroke_sink = stroke_geo.Open()?;

            fill_sink.BeginFigure(
                D2D_POINT_2F {
                    x: rect.left,
                    y: rect.bottom,
                },
                windows::Win32::Graphics::Direct2D::Common::D2D1_FIGURE_BEGIN_FILLED,
            );
            fill_sink.AddLine(pts[0]);
            stroke_sink.BeginFigure(
                pts[0],
                windows::Win32::Graphics::Direct2D::Common::D2D1_FIGURE_BEGIN_HOLLOW,
            );

            for i in 0..pts.len() - 1 {
                let p0 = if i == 0 { pts[0] } else { pts[i - 1] };
                let p1 = pts[i];
                let p2 = pts[i + 1];
                let p3 = if i + 2 < pts.len() {
                    pts[i + 2]
                } else {
                    pts[pts.len() - 1]
                };
                let seg = D2D1_BEZIER_SEGMENT {
                    point1: D2D_POINT_2F {
                        x: p1.x + (p2.x - p0.x) / 6.0,
                        y: (p1.y + (p2.y - p0.y) / 6.0).clamp(rect.top + 1.5, rect.bottom - 1.5),
                    },
                    point2: D2D_POINT_2F {
                        x: p2.x - (p3.x - p1.x) / 6.0,
                        y: (p2.y - (p3.y - p1.y) / 6.0).clamp(rect.top + 1.5, rect.bottom - 1.5),
                    },
                    point3: p2,
                };
                fill_sink.AddBezier(&seg);
                stroke_sink.AddBezier(&seg);
            }

            fill_sink.AddLine(D2D_POINT_2F {
                x: rect.right,
                y: rect.bottom,
            });
            fill_sink.EndFigure(
                windows::Win32::Graphics::Direct2D::Common::D2D1_FIGURE_END_CLOSED,
            );
            stroke_sink.EndFigure(
                windows::Win32::Graphics::Direct2D::Common::D2D1_FIGURE_END_OPEN,
            );
            fill_sink.Close()?;
            stroke_sink.Close()?;

            let stops = [
                D2D1_GRADIENT_STOP {
                    position: 0.0,
                    color: D2D1_COLOR_F {
                        a: 0.30,
                        ..stroke_color
                    },
                },
                D2D1_GRADIENT_STOP {
                    position: 0.55,
                    color: D2D1_COLOR_F {
                        a: 0.09,
                        ..stroke_color
                    },
                },
                D2D1_GRADIENT_STOP {
                    position: 1.0,
                    color: D2D1_COLOR_F {
                        a: 0.0,
                        ..stroke_color
                    },
                },
            ];
            let stop_coll = self.dc.CreateGradientStopCollection(
                &stops,
                D2D1_COLOR_SPACE_SRGB,
                D2D1_COLOR_SPACE_SRGB,
                D2D1_BUFFER_PRECISION_8BPC_UNORM,
                D2D1_EXTEND_MODE_CLAMP,
                D2D1_COLOR_INTERPOLATION_MODE_PREMULTIPLIED,
            )?;
            let fill_brush: ID2D1LinearGradientBrush = self.dc.CreateLinearGradientBrush(
                &D2D1_LINEAR_GRADIENT_BRUSH_PROPERTIES {
                    startPoint: D2D_POINT_2F {
                        x: 0.0,
                        y: rect.top,
                    },
                    endPoint: D2D_POINT_2F {
                        x: 0.0,
                        y: rect.bottom,
                    },
                },
                None,
                &stop_coll,
            )?;
            self.dc.FillGeometry(&fill_geo, &fill_brush, None);

            let line_brush = self.solid(stroke_color);
            self.dc.DrawGeometry(&stroke_geo, line_brush, 1.6, None);

            if let Some(last) = pts.last() {
                let halo_brush = self.solid(D2D1_COLOR_F {
                    a: 0.28,
                    ..stroke_color
                });
                self.dc.FillEllipse(
                    &D2D1_ELLIPSE {
                        point: *last,
                        radiusX: 4.5,
                        radiusY: 4.5,
                    },
                    halo_brush,
                );
                let dot_brush = self.solid(stroke_color);
                self.dc.FillEllipse(
                    &D2D1_ELLIPSE {
                        point: *last,
                        radiusX: 2.1,
                        radiusY: 2.1,
                    },
                    dot_brush,
                );
            }
        }
        let _ = (D2D1_ALPHA_MODE_PREMULTIPLIED, D2D1_FEATURE_LEVEL_DEFAULT);
        Ok(())
    }

    /// Draws an `exelban/stats`-style circular ring gauge (`progress` in `0.0..=1.0`, starting at 12 o'clock).
    #[allow(clippy::too_many_arguments)]
    pub fn draw_ring_gauge(
        &self,
        cx: f32,
        cy: f32,
        radius: f32,
        thickness: f32,
        progress: f32,
        track_color: D2D1_COLOR_F,
        fill_color: D2D1_COLOR_F,
    ) -> windows::core::Result<()> {
        let track_brush = self.solid(track_color);
        unsafe {
            self.dc.DrawEllipse(
                &D2D1_ELLIPSE {
                    point: D2D_POINT_2F { x: cx, y: cy },
                    radiusX: radius,
                    radiusY: radius,
                },
                track_brush,
                thickness,
                None,
            );
        }

        let clamped = progress.clamp(0.02, 1.0);
        let steps = 64usize;
        let start_angle = -std::f32::consts::FRAC_PI_2;
        let sweep = clamped * std::f32::consts::TAU;

        unsafe {
            let arc_geo = self.d2d_factory.CreatePathGeometry()?;
            let sink = arc_geo.Open()?;
            let x0 = cx + radius * start_angle.cos();
            let y0 = cy + radius * start_angle.sin();
            sink.BeginFigure(
                D2D_POINT_2F { x: x0, y: y0 },
                windows::Win32::Graphics::Direct2D::Common::D2D1_FIGURE_BEGIN_HOLLOW,
            );
            for i in 1..=steps {
                let t = i as f32 / steps as f32;
                let ang = start_angle + sweep * t;
                sink.AddLine(D2D_POINT_2F {
                    x: cx + radius * ang.cos(),
                    y: cy + radius * ang.sin(),
                });
            }
            sink.EndFigure(windows::Win32::Graphics::Direct2D::Common::D2D1_FIGURE_END_OPEN);
            sink.Close()?;

            let stroke_props = D2D1_STROKE_STYLE_PROPERTIES1 {
                startCap: D2D1_CAP_STYLE_ROUND,
                endCap: D2D1_CAP_STYLE_ROUND,
                ..Default::default()
            };
            let stroke_style = self.d2d_factory.CreateStrokeStyle(&stroke_props, None)?;
            let fill_brush = self.solid(fill_color);
            self.dc
                .DrawGeometry(&arc_geo, fill_brush, thickness, &stroke_style);
        }
        Ok(())
    }

    /// Draws `exelban/stats`'s signature centered section header flanked by subtle hairlines (`─── LABEL ───`).
    #[allow(clippy::too_many_arguments)]
    pub fn draw_section_divider(
        &self,
        x1: f32,
        x2: f32,
        y: f32,
        label: &str,
        fmt: &IDWriteTextFormat,
    ) -> windows::core::Result<()> {
        let mid = (x1 + x2) * 0.5;
        let est_w = (label.chars().count() as f32 * 5.8 + 16.0).min((x2 - x1) * 0.65);
        let left_end = (mid - est_w * 0.5).max(x1);
        let right_start = (mid + est_w * 0.5).min(x2);

        if left_end > x1 + 4.0 {
            self.draw_hairline(x1, y, left_end)?;
        }
        if x2 > right_start + 4.0 {
            self.draw_hairline(right_start, y, x2)?;
        }
        self.draw_text(
            label,
            fmt,
            D2D_RECT_F {
                left: left_end,
                top: y - 8.0,
                right: right_start,
                bottom: y + 8.0,
            },
            TEXT_MUTED,
            false,
        )?;
        Ok(())
    }

    /// Draws an `exelban/stats`-style multi-segment horizontal allocation bar.
    pub fn draw_segmented_bar(
        &self,
        rect: D2D_RECT_F,
        segments: &[(f32, D2D1_COLOR_F)],
        track_color: D2D1_COLOR_F,
    ) -> windows::core::Result<()> {
        let h = (rect.bottom - rect.top).max(2.0);
        let total_w = (rect.right - rect.left).max(1.0);
        let r = h * 0.5;
        self.fill_rounded(rect, r, track_color)?;

        let sum: f32 = segments.iter().map(|(v, _)| v.max(0.0)).sum();
        if sum <= 0.0 {
            return Ok(());
        }

        let mut cursor = rect.left;
        let gap = 1.5f32;
        for (idx, &(val, col)) in segments.iter().enumerate() {
            if val <= 0.0 {
                continue;
            }
            let seg_w = (total_w * (val / sum)).max(3.0);
            let right = if idx + 1 == segments.len() {
                rect.right
            } else {
                (cursor + seg_w - gap).min(rect.right)
            };
            if right > cursor + 1.0 {
                self.fill_rounded(
                    D2D_RECT_F {
                        left: cursor,
                        top: rect.top,
                        right,
                        bottom: rect.bottom,
                    },
                    r.min((right - cursor) * 0.5),
                    col,
                )?;
            }
            cursor = right + gap;
            if cursor >= rect.right {
                break;
            }
        }
        Ok(())
    }

    /// Draws a horizontal pill progress bar (`progress` in `0.0..=1.0`).
    pub fn draw_progress_bar(
        &self,
        rect: D2D_RECT_F,
        progress: f32,
        track_color: D2D1_COLOR_F,
        fill_color: D2D1_COLOR_F,
    ) -> windows::core::Result<()> {
        let h = (rect.bottom - rect.top).max(1.0);
        let w = (rect.right - rect.left).max(1.0);
        let r = h * 0.5;
        self.fill_rounded(rect, r, track_color)?;
        let fill_w = (w * progress.clamp(0.06, 1.0)).max(h);
        let fill_rect = D2D_RECT_F {
            left: rect.left,
            top: rect.top,
            right: (rect.left + fill_w).min(rect.right),
            bottom: rect.bottom,
        };
        self.fill_rounded(fill_rect, r, fill_color)?;
        Ok(())
    }

    /// Draws text with a subtle 1px dark backdrop shadow for readability in transparent HUD mode.
    pub fn draw_text_hud(
        &self,
        text: &str,
        fmt: &IDWriteTextFormat,
        rect: D2D_RECT_F,
        color: D2D1_COLOR_F,
        tabular: bool,
        shadow: bool,
    ) -> windows::core::Result<()> {
        if shadow {
            let srect = D2D_RECT_F {
                left: rect.left + 0.8,
                top: rect.top + 0.8,
                right: rect.right + 0.8,
                bottom: rect.bottom + 0.8,
            };
            self.draw_text(text, fmt, srect, rgba(0, 0, 0, 0.72), tabular)?;
        }
        self.draw_text(text, fmt, rect, color, tabular)
    }

    /// Draws an `exelban/stats`-style mini bar-history chart (ascending columns bottom-up).
    pub fn draw_mini_bars(
        &self,
        rect: D2D_RECT_F,
        samples: &[u8],
        max_val: f32,
        bar_color: D2D1_COLOR_F,
        track_color: D2D1_COLOR_F,
    ) -> windows::core::Result<()> {
        if samples.is_empty() {
            return Ok(());
        }
        let w = (rect.right - rect.left).max(4.0);
        let h = (rect.bottom - rect.top).max(4.0);
        let n = samples.len() as f32;
        let gap = 1.2_f32;
        let bar_w = ((w - gap * (n - 1.0)) / n).max(1.5);
        let ceil = max_val.max(1.0);

        for (i, &val) in samples.iter().enumerate() {
            let x = rect.left + i as f32 * (bar_w + gap);
            let ratio = (val as f32 / ceil).clamp(0.08, 1.0);
            let bh = (h * ratio).max(2.0);
            self.fill_rounded(
                D2D_RECT_F {
                    left: x,
                    top: rect.top,
                    right: x + bar_w,
                    bottom: rect.bottom,
                },
                0.8,
                track_color,
            )?;
            self.fill_rounded(
                D2D_RECT_F {
                    left: x,
                    top: rect.bottom - bh,
                    right: x + bar_w,
                    bottom: rect.bottom,
                },
                0.8,
                bar_color,
            )?;
        }
        Ok(())
    }

    /// Draws a slim vertical gauge bar filling from bottom to top (used on `RightEdge` rail).
    pub fn draw_vertical_bar(
        &self,
        rect: D2D_RECT_F,
        progress: f32,
        track_color: D2D1_COLOR_F,
        fill_color: D2D1_COLOR_F,
    ) -> windows::core::Result<()> {
        let w = (rect.right - rect.left).max(2.0);
        let h = (rect.bottom - rect.top).max(4.0);
        let r = (w * 0.5).min(3.0);
        self.fill_rounded(rect, r, track_color)?;
        let fill_h = (h * progress.clamp(0.06, 1.0)).max(w);
        self.fill_rounded(
            D2D_RECT_F {
                left: rect.left,
                top: (rect.bottom - fill_h).max(rect.top),
                right: rect.right,
                bottom: rect.bottom,
            },
            r,
            fill_color,
        )?;
        Ok(())
    }
}

#[inline]
fn identity_matrix() -> windows::Foundation::Numerics::Matrix3x2 {
    windows::Foundation::Numerics::Matrix3x2 {
        M11: 1.0,
        M12: 0.0,
        M21: 0.0,
        M22: 1.0,
        M31: 0.0,
        M32: 0.0,
    }
}
