use std::mem;
use std::ptr::null_mut;
use std::sync::atomic::Ordering::Relaxed;

use crate::api::overlay::color::{BAD_COLOR, percent_to_color};
use crate::api::overlay::{BACKGROUND_COLOR, CONNECTED, FONT_COLOR, FONT_HEIGHT, LATENCY, Result};
use log::{error, info};
use widestring::U16CString;
use windows::Win32::Foundation::{
    COLORREF, HINSTANCE, HWND, LPARAM, LRESULT, POINT, RECT, SIZE, WPARAM,
};
use windows::Win32::Graphics::Gdi::{
    AC_SRC_ALPHA, AC_SRC_OVER, BLENDFUNCTION, BeginPaint, CreateCompatibleDC, DeleteDC,
    DeleteObject, EndPaint, GetDC, HBITMAP, HBRUSH, ReleaseDC, SelectObject, UpdateWindow,
};
use windows::Win32::Graphics::GdiPlus::{
    GdipCreateBitmapFromScan0, GdipCreateFont, GdipCreateHBITMAPFromBitmap, GdipCreateSolidFill,
    GdipDeleteBrush, GdipDeleteFont, GdipDeleteFontFamily, GdipDeleteGraphics,
    GdipDeleteStringFormat, GdipDisposeImage, GdipDrawString, GdipFillRectangle,
    GdipGetFontCollectionFamilyList, GdipGetImageGraphicsContext, GdipMeasureString,
    GdipNewPrivateFontCollection, GdipPrivateAddMemoryFont, GdipSetTextRenderingHint,
    GdipStringFormatGetGenericDefault, GdiplusStartup, GdiplusStartupInput, GpFont,
    GpFontCollection, GpFontFamily, GpGraphics, GpStringFormat, RectF, TextRenderingHint, Unit,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleA;
use windows::Win32::UI::WindowsAndMessaging::{
    CS_HREDRAW, CS_VREDRAW, CreateWindowExW, DefWindowProcW, DestroyWindow, GetClientRect, HCURSOR,
    HICON, HWND_TOPMOST, PostQuitMessage, RegisterClassW, SW_HIDE, SWP_NOMOVE, SWP_NOSIZE,
    SetWindowPos, ShowWindow, ULW_ALPHA, UpdateLayeredWindow, WNDCLASSW, WS_EX_LAYERED,
    WS_EX_TOOLWINDOW, WS_EX_TRANSPARENT, WS_POPUP, WS_VISIBLE,
};
use windows::core::{BOOL, PCWSTR};

pub(crate) const CLASS_NAME: &str = "telepathy_overlay";
const FONT_BYTES: &[u8] = include_bytes!("../../../../../assets/Inconsolata.ttf");

pub(crate) unsafe fn build_window(width: i32, height: i32, x: i32, y: i32) -> Result<HWND> {
    unsafe {
        let input: GdiplusStartupInput = GdiplusStartupInput {
            GdiplusVersion: 1,
            DebugEventCallback: mem::zeroed(),
            SuppressBackgroundThread: BOOL(0),
            SuppressExternalCodecs: BOOL(0),
        };

        let mut token = 0;
        let status = GdiplusStartup(&mut token, &input, null_mut());
        info!("GdiplusStartup status: {:?}", status);

        let class_name = U16CString::from_str(CLASS_NAME)?;
        let window_name = U16CString::from_str("Overlay")?;

        let h_module = GetModuleHandleA(None)?;
        let h_instance = HINSTANCE::from(h_module);

        let wc = WNDCLASSW {
            style: CS_HREDRAW | CS_VREDRAW,
            lpfnWndProc: Some(window_proc),
            cbClsExtra: 0,
            cbWndExtra: 0,
            hInstance: h_instance,
            hIcon: HICON::default(),
            hCursor: HCURSOR::default(),
            hbrBackground: HBRUSH::default(),
            lpszMenuName: PCWSTR::null(),
            lpszClassName: PCWSTR::from_raw(class_name.as_ptr()),
        };

        RegisterClassW(&wc);

        let hwnd = CreateWindowExW(
            WS_EX_LAYERED | WS_EX_TRANSPARENT | WS_EX_TOOLWINDOW,
            Some(&PCWSTR::from_raw(class_name.as_ptr())),
            Some(&PCWSTR::from_raw(window_name.as_ptr())),
            WS_POPUP | WS_VISIBLE,
            x,
            y,
            width,
            height,
            None,
            None,
            Some(h_instance),
            None,
        )?;

        SetWindowPos(
            hwnd,
            Some(HWND_TOPMOST),
            x,
            y,
            width,
            height,
            SWP_NOMOVE | SWP_NOSIZE,
        )?;

        _ = ShowWindow(hwnd, SW_HIDE);
        _ = UpdateWindow(hwnd);

        Ok(hwnd)
    }
}

unsafe extern "system" fn window_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    unsafe {
        match msg {
            1 => LRESULT(0),
            15 => {
                let mut info = mem::zeroed();
                let _hdc = BeginPaint(hwnd, &mut info);

                draw_overlay(hwnd);

                _ = EndPaint(hwnd, &info);
                LRESULT(0)
            }
            16 => {
                _ = DestroyWindow(hwnd);
                LRESULT(0)
            }
            2 => {
                PostQuitMessage(0);
                LRESULT(0)
            }
            _ => DefWindowProcW(hwnd, msg, wparam, lparam),
        }
    }
}

unsafe fn draw_overlay(hwnd: HWND) {
    unsafe {
        let hdc_screen = GetDC(None);
        let hdc_mem = CreateCompatibleDC(Some(hdc_screen));

        let mut rect: RECT = mem::zeroed();
        _ = GetClientRect(hwnd, &mut rect);

        let width = rect.right - rect.left;
        let height = rect.bottom - rect.top;

        let mut bitmap = null_mut();
        GdipCreateBitmapFromScan0(width, height, 0, 925707, None, &mut bitmap);

        let mut graphics = null_mut();
        GdipGetImageGraphicsContext(bitmap.cast(), &mut graphics);
        GdipSetTextRenderingHint(graphics, TextRenderingHint(4));

        let mut background_brush = null_mut();
        GdipCreateSolidFill(BACKGROUND_COLOR.load(Relaxed), &mut background_brush);
        GdipFillRectangle(
            graphics,
            background_brush.cast(),
            0.0,
            0.0,
            width as f32,
            height as f32,
        );

        let mut font_collection: *mut GpFontCollection = null_mut();
        let status = GdipNewPrivateFontCollection(&mut font_collection);

        if status.0 != 0 {
            error!("failed to create private font collection: {}", status.0);
            return;
        }

        let font_size = FONT_BYTES.len() as i32;
        let font_ptr = FONT_BYTES.as_ptr() as *const std::ffi::c_void;

        let status = GdipPrivateAddMemoryFont(font_collection, font_ptr, font_size);
        if status.0 != 0 {
            error!("failed to add memory font: {}", status.0);
            return;
        }

        let mut font_families: [*mut GpFontFamily; 1] = [null_mut()];
        let mut found: i32 = 0;
        let status =
            GdipGetFontCollectionFamilyList(font_collection, &mut font_families, &mut found);
        if status.0 != 0 || found == 0 {
            error!("failed to retrieve font family {}", status.0);
            return;
        }

        let font_family = font_families[0];

        let mut font = null_mut();
        GdipCreateFont(
            font_family,
            FONT_HEIGHT.load(Relaxed) as f32,
            0,
            Unit(0),
            &mut font,
        );

        let mut string_format = null_mut();
        GdipStringFormatGetGenericDefault(&mut string_format);

        let mut bounding = draw_text(
            "Latency:",
            (0.0, 0.0),
            FONT_COLOR.load(Relaxed),
            graphics,
            font,
            string_format,
        );

        let latency = LATENCY.load(Relaxed);
        let color = percent_to_color(latency as f64 / 200.0);
        bounding = draw_text(
            &latency.to_string(),
            (bounding.Width, 0.0),
            color.argb(),
            graphics,
            font,
            string_format,
        );

        // TODO re-enable loss in the overlay
        // bounding = draw_text(
        //     "Loss:",
        //     (bounding.X + bounding.Width + 30.0, 0.0),
        //     FONT_COLOR.load(Relaxed),
        //     graphics,
        //     font,
        //     string_format,
        // );
        //
        // let loss = LOSS.load(Relaxed);
        // let color = percent_to_color(loss);
        // bounding = draw_text(
        //     &format!("{:.2}%", loss * 100.0),
        //     (bounding.X + bounding.Width, 0.0),
        //     color.argb(),
        //     graphics,
        //     font,
        //     string_format,
        // );

        if !CONNECTED.load(Relaxed) {
            _ = draw_text(
                "Disconnected",
                (bounding.X + bounding.Width + 30.0, 0.0),
                BAD_COLOR.argb(),
                graphics,
                font,
                string_format,
            );
        }

        let mut h_bitmap = HBITMAP::default();
        GdipCreateHBITMAPFromBitmap(bitmap, &mut h_bitmap, 0);

        let old_bitmap = SelectObject(hdc_mem, h_bitmap.into());

        let point_source = POINT { x: 0, y: 0 };
        let size = SIZE {
            cx: width,
            cy: height,
        };
        let blend = BLENDFUNCTION {
            BlendOp: AC_SRC_OVER as u8,
            BlendFlags: 0,
            SourceConstantAlpha: 255,
            AlphaFormat: AC_SRC_ALPHA as u8,
        };

        _ = UpdateLayeredWindow(
            hwnd,
            Some(hdc_screen),
            None,
            Some(&size),
            Some(hdc_mem),
            Some(&point_source),
            COLORREF(0),
            Some(&blend),
            ULW_ALPHA,
        );

        SelectObject(hdc_mem, old_bitmap);
        _ = DeleteObject(h_bitmap.into());
        _ = DeleteDC(hdc_mem);
        GdipDeleteFontFamily(font_family);
        GdipDeleteFont(font);
        GdipDeleteStringFormat(string_format);
        GdipDeleteGraphics(graphics);
        GdipDisposeImage(bitmap.cast());
        GdipDeleteBrush(background_brush.cast());
        ReleaseDC(None, hdc_screen);
    }
}

unsafe fn draw_text(
    text: &str,
    position: (f32, f32),
    color: u32,
    graphics: *mut GpGraphics,
    font: *mut GpFont,
    string_format: *mut GpStringFormat,
) -> RectF {
    unsafe {
        let mut brush = null_mut();
        GdipCreateSolidFill(color, &mut brush);

        let point_f = RectF {
            X: position.0,
            Y: position.1,
            Width: 0.0,
            Height: 0.0,
        };

        let message = U16CString::from_str(text).unwrap();

        let mut bounding_box = mem::zeroed();
        GdipMeasureString(
            graphics,
            Some(&PCWSTR::from_raw(message.as_ptr())),
            -1,
            font,
            &point_f,
            string_format,
            &mut bounding_box,
            null_mut(),
            null_mut(),
        );

        GdipDrawString(
            graphics,
            Some(&PCWSTR::from_raw(message.as_ptr())),
            -1,
            font,
            &point_f,
            string_format,
            brush.cast(),
        );

        GdipDeleteBrush(brush.cast());
        bounding_box
    }
}
