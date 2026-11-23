//! Direct3D 11, DXGI, Direct2D 1.1, and DirectWrite device/factory management.

use crate::render::theme::{FONT_DISPLAY, FONT_MONO, FONT_TABULAR, FONT_UI};
use windows::core::{Interface, HSTRING};
use windows::Win32::Graphics::Direct2D::{
    D2D1CreateFactory, ID2D1Device, ID2D1Factory1, ID2D1StrokeStyle1, D2D1_CAP_STYLE_ROUND,
    D2D1_FACTORY_OPTIONS, D2D1_FACTORY_TYPE_SINGLE_THREADED, D2D1_STROKE_STYLE_PROPERTIES1,
};
use windows::Win32::Graphics::Direct3D::{
    D3D_DRIVER_TYPE_HARDWARE, D3D_DRIVER_TYPE_WARP, D3D_FEATURE_LEVEL_10_0,
    D3D_FEATURE_LEVEL_11_0, D3D_FEATURE_LEVEL_11_1,
};
use windows::Win32::Graphics::Direct3D11::{
    D3D11CreateDevice, ID3D11Device, D3D11_CREATE_DEVICE_BGRA_SUPPORT, D3D11_SDK_VERSION,
};
use windows::Win32::Graphics::DirectWrite::{
    DWriteCreateFactory, IDWriteFactory, IDWriteTextFormat, IDWriteTypography,
    DWRITE_FONT_FEATURE, DWRITE_FONT_FEATURE_TAG_TABULAR_FIGURES, DWRITE_FONT_STRETCH_NORMAL,
    DWRITE_FONT_STYLE_NORMAL, DWRITE_FONT_WEIGHT_MEDIUM, DWRITE_FONT_WEIGHT_NORMAL,
    DWRITE_FONT_WEIGHT_SEMI_BOLD, DWRITE_PARAGRAPH_ALIGNMENT_CENTER,
    DWRITE_TEXT_ALIGNMENT_CENTER, DWRITE_TEXT_ALIGNMENT_LEADING, DWRITE_TEXT_ALIGNMENT_TRAILING,
    DWRITE_TRIMMING, DWRITE_TRIMMING_GRANULARITY_CHARACTER, DWRITE_WORD_WRAPPING_NO_WRAP,
    DWRITE_WORD_WRAPPING_WRAP,
};
use windows::Win32::Graphics::Dxgi::IDXGIDevice3;

pub struct RenderDevice {
    pub d3d_device: ID3D11Device,
    pub dxgi_device: IDXGIDevice3,
    pub d2d_factory: ID2D1Factory1,
    pub d2d_device: ID2D1Device,
    pub round_stroke_style: ID2D1StrokeStyle1,
    pub dwrite_factory: IDWriteFactory,
    pub fonts: FontSet,
}

pub struct FontSet {
    pub pill_label: IDWriteTextFormat,
    pub pill_mono: IDWriteTextFormat,
    pub gauge_val: IDWriteTextFormat,
    pub gauge_lbl: IDWriteTextFormat,
    pub title: IDWriteTextFormat,
    pub title_center: IDWriteTextFormat,
    pub section_hdr: IDWriteTextFormat,
    pub section_hdr_center: IDWriteTextFormat,
    pub row_primary: IDWriteTextFormat,
    pub row_secondary: IDWriteTextFormat,
    pub caption_center: IDWriteTextFormat,
    pub mono_metric: IDWriteTextFormat,
    pub mono_badge: IDWriteTextFormat,
    pub micro_lbl: IDWriteTextFormat,
    pub term_line: IDWriteTextFormat,
    pub tabular_typography: IDWriteTypography,
}

impl RenderDevice {
    pub fn new() -> windows::core::Result<Self> {
        let (d3d_device, dxgi_device) = create_d3d_device()?;
        // SAFETY: Initialized Direct2D/DirectWrite COM interfaces and valid parameters.
        let d2d_factory: ID2D1Factory1 = unsafe {
            D2D1CreateFactory(
                D2D1_FACTORY_TYPE_SINGLE_THREADED,
                Some(&D2D1_FACTORY_OPTIONS::default()),
            )?
        };
        // SAFETY: Initialized Direct2D/DirectWrite COM interfaces and valid parameters.
        let d2d_device = unsafe { d2d_factory.CreateDevice(&dxgi_device)? };
        let stroke_props = D2D1_STROKE_STYLE_PROPERTIES1 {
            startCap: D2D1_CAP_STYLE_ROUND,
            endCap: D2D1_CAP_STYLE_ROUND,
            ..Default::default()
        };
        // SAFETY: `d2d_factory` is valid and stroke properties are initialized.
        let round_stroke_style = unsafe { d2d_factory.CreateStrokeStyle(&stroke_props, None)? };
        // SAFETY: Initialized Direct2D/DirectWrite COM interfaces and valid parameters.
        let dwrite_factory: IDWriteFactory = unsafe {
            DWriteCreateFactory(windows::Win32::Graphics::DirectWrite::DWRITE_FACTORY_TYPE_SHARED)?
        };
        let fonts = FontSet::new(&dwrite_factory)?;
        Ok(Self {
            d3d_device,
            dxgi_device,
            d2d_factory,
            d2d_device,
            round_stroke_style,
            dwrite_factory,
            fonts,
        })
    }

    /// Rebuilds GPU device resources after `DXGI_ERROR_DEVICE_REMOVED`.
    pub fn recreate(&mut self) -> windows::core::Result<()> {
        let (d3d_device, dxgi_device) = create_d3d_device()?;
        // SAFETY: Initialized Direct2D/DirectWrite COM interfaces and valid parameters.
        let d2d_device = unsafe { self.d2d_factory.CreateDevice(&dxgi_device)? };
        self.d3d_device = d3d_device;
        self.dxgi_device = dxgi_device;
        self.d2d_device = d2d_device;
        Ok(())
    }

    /// Returns temporary DXGI/D3D driver caches to the OS (used after collapsing).
    pub fn trim(&self) {
        // SAFETY: Initialized Direct2D/DirectWrite COM interfaces and valid parameters.
        unsafe {
            self.dxgi_device.Trim();
        }
    }
}

fn create_d3d_device() -> windows::core::Result<(ID3D11Device, IDXGIDevice3)> {
    let levels = [
        D3D_FEATURE_LEVEL_11_1,
        D3D_FEATURE_LEVEL_11_0,
        D3D_FEATURE_LEVEL_10_0,
    ];
    let mut device: Option<ID3D11Device> = None;

    // SAFETY: Initialized Direct2D/DirectWrite COM interfaces and valid parameters.
    let hr = unsafe {
        D3D11CreateDevice(
            None,
            D3D_DRIVER_TYPE_HARDWARE,
            None,
            D3D11_CREATE_DEVICE_BGRA_SUPPORT,
            Some(&levels),
            D3D11_SDK_VERSION,
            Some(&mut device),
            None,
            None,
        )
    };

    if hr.is_err() || device.is_none() {
        // SAFETY: Initialized Direct2D/DirectWrite COM interfaces and valid parameters.
        unsafe {
            D3D11CreateDevice(
                None,
                D3D_DRIVER_TYPE_WARP,
                None,
                D3D11_CREATE_DEVICE_BGRA_SUPPORT,
                Some(&levels),
                D3D11_SDK_VERSION,
                Some(&mut device),
                None,
                None,
            )?;
        }
    }

    let d3d_device = device.expect("D3D11CreateDevice succeeded without device");
    let dxgi_device: IDXGIDevice3 = d3d_device.cast()?;
    Ok((d3d_device, dxgi_device))
}

impl FontSet {
    fn new(dw: &IDWriteFactory) -> windows::core::Result<Self> {
        let pill_label = make_format(
            dw,
            FONT_UI,
            DWRITE_FONT_WEIGHT_MEDIUM,
            12.0,
            DWRITE_TEXT_ALIGNMENT_LEADING,
            DWRITE_PARAGRAPH_ALIGNMENT_CENTER,
            true,
        )?;
        let pill_mono = make_format(
            dw,
            FONT_TABULAR,
            DWRITE_FONT_WEIGHT_SEMI_BOLD,
            12.0,
            DWRITE_TEXT_ALIGNMENT_LEADING,
            DWRITE_PARAGRAPH_ALIGNMENT_CENTER,
            true,
        )?;
        let gauge_val = make_format(
            dw,
            FONT_DISPLAY,
            DWRITE_FONT_WEIGHT_SEMI_BOLD,
            13.5,
            DWRITE_TEXT_ALIGNMENT_CENTER,
            DWRITE_PARAGRAPH_ALIGNMENT_CENTER,
            true,
        )?;
        let gauge_lbl = make_format(
            dw,
            FONT_UI,
            DWRITE_FONT_WEIGHT_MEDIUM,
            9.0,
            DWRITE_TEXT_ALIGNMENT_CENTER,
            DWRITE_PARAGRAPH_ALIGNMENT_CENTER,
            true,
        )?;
        let title = make_format(
            dw,
            FONT_DISPLAY,
            DWRITE_FONT_WEIGHT_SEMI_BOLD,
            13.0,
            DWRITE_TEXT_ALIGNMENT_LEADING,
            DWRITE_PARAGRAPH_ALIGNMENT_CENTER,
            true,
        )?;
        let title_center = make_format(
            dw,
            FONT_DISPLAY,
            DWRITE_FONT_WEIGHT_SEMI_BOLD,
            13.0,
            DWRITE_TEXT_ALIGNMENT_CENTER,
            DWRITE_PARAGRAPH_ALIGNMENT_CENTER,
            true,
        )?;
        let section_hdr = make_format(
            dw,
            FONT_UI,
            DWRITE_FONT_WEIGHT_SEMI_BOLD,
            9.5,
            DWRITE_TEXT_ALIGNMENT_LEADING,
            DWRITE_PARAGRAPH_ALIGNMENT_CENTER,
            true,
        )?;
        let section_hdr_center = make_format(
            dw,
            FONT_UI,
            DWRITE_FONT_WEIGHT_SEMI_BOLD,
            9.5,
            DWRITE_TEXT_ALIGNMENT_CENTER,
            DWRITE_PARAGRAPH_ALIGNMENT_CENTER,
            true,
        )?;
        let row_primary = make_format(
            dw,
            FONT_UI,
            DWRITE_FONT_WEIGHT_MEDIUM,
            12.5,
            DWRITE_TEXT_ALIGNMENT_LEADING,
            DWRITE_PARAGRAPH_ALIGNMENT_CENTER,
            true,
        )?;
        let row_secondary = make_format(
            dw,
            FONT_UI,
            DWRITE_FONT_WEIGHT_NORMAL,
            10.5,
            DWRITE_TEXT_ALIGNMENT_LEADING,
            DWRITE_PARAGRAPH_ALIGNMENT_CENTER,
            true,
        )?;
        let caption_center = make_format(
            dw,
            FONT_UI,
            DWRITE_FONT_WEIGHT_NORMAL,
            10.5,
            DWRITE_TEXT_ALIGNMENT_CENTER,
            DWRITE_PARAGRAPH_ALIGNMENT_CENTER,
            true,
        )?;
        let mono_metric = make_format(
            dw,
            FONT_TABULAR,
            DWRITE_FONT_WEIGHT_MEDIUM,
            11.5,
            DWRITE_TEXT_ALIGNMENT_TRAILING,
            DWRITE_PARAGRAPH_ALIGNMENT_CENTER,
            true,
        )?;
        let mono_badge = make_format(
            dw,
            FONT_TABULAR,
            DWRITE_FONT_WEIGHT_MEDIUM,
            10.5,
            DWRITE_TEXT_ALIGNMENT_CENTER,
            DWRITE_PARAGRAPH_ALIGNMENT_CENTER,
            true,
        )?;
        let micro_lbl = make_format(
            dw,
            FONT_UI,
            DWRITE_FONT_WEIGHT_SEMI_BOLD,
            8.0,
            DWRITE_TEXT_ALIGNMENT_CENTER,
            DWRITE_PARAGRAPH_ALIGNMENT_CENTER,
            true,
        )?;
        let term_line = make_format(
            dw,
            FONT_MONO,
            DWRITE_FONT_WEIGHT_NORMAL,
            10.5,
            DWRITE_TEXT_ALIGNMENT_LEADING,
            DWRITE_PARAGRAPH_ALIGNMENT_CENTER,
            true,
        )?;
        // SAFETY: Initialized Direct2D/DirectWrite COM interfaces and valid parameters.
        let tabular_typography = unsafe { dw.CreateTypography()? };
        // SAFETY: Initialized Direct2D/DirectWrite COM interfaces and valid parameters.
        unsafe {
            tabular_typography.AddFontFeature(DWRITE_FONT_FEATURE {
                nameTag: DWRITE_FONT_FEATURE_TAG_TABULAR_FIGURES,
                parameter: 1,
            })?;
        }

        Ok(Self {
            pill_label,
            pill_mono,
            gauge_val,
            gauge_lbl,
            title,
            title_center,
            section_hdr,
            section_hdr_center,
            row_primary,
            row_secondary,
            caption_center,
            mono_metric,
            mono_badge,
            micro_lbl,
            term_line,
            tabular_typography,
        })
    }
}

fn make_format(
    dw: &IDWriteFactory,
    family: &str,
    weight: windows::Win32::Graphics::DirectWrite::DWRITE_FONT_WEIGHT,
    size: f32,
    halign: windows::Win32::Graphics::DirectWrite::DWRITE_TEXT_ALIGNMENT,
    valign: windows::Win32::Graphics::DirectWrite::DWRITE_PARAGRAPH_ALIGNMENT,
    no_wrap: bool,
) -> windows::core::Result<IDWriteTextFormat> {
    let locale = HSTRING::from("");
    let fam = HSTRING::from(family);
    // SAFETY: Initialized Direct2D/DirectWrite COM interfaces and valid parameters.
    unsafe {
        let fmt = dw.CreateTextFormat(
            &fam,
            None,
            weight,
            DWRITE_FONT_STYLE_NORMAL,
            DWRITE_FONT_STRETCH_NORMAL,
            size,
            &locale,
        )?;
        fmt.SetTextAlignment(halign)?;
        fmt.SetParagraphAlignment(valign)?;
        fmt.SetWordWrapping(if no_wrap {
            DWRITE_WORD_WRAPPING_NO_WRAP
        } else {
            DWRITE_WORD_WRAPPING_WRAP
        })?;
        if no_wrap {
            let trimming = DWRITE_TRIMMING {
                granularity: DWRITE_TRIMMING_GRANULARITY_CHARACTER,
                delimiter: 0,
                delimiterCount: 0,
            };
            let ellipsis = dw.CreateEllipsisTrimmingSign(&fmt)?;
            fmt.SetTrimming(&trimming, &ellipsis)?;
        }
        Ok(fmt)
    }
}
