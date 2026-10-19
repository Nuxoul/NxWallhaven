//! Pointer input regression for the actual native detail component.
use super::*;
#[test]
fn detail_wheel_keeps_the_image_point_under_the_cursor_stationary() {
    i_slint_backend_testing::init_no_event_loop();
    let ui = DetailWindow::new().unwrap();
    ui.window().set_size(slint::LogicalSize::new(1120.0, 760.0));
    ui.set_image_loaded(true);
    ui.set_image_busy(false);
    ui.show().unwrap();
    slint::platform::update_timers_and_animations();
    let w = ui.get_viewport_width();
    let h = ui.get_viewport_height();
    // Detail has a fixed 40px caption and 12px content inset. Local cursor is 3/4 width, 1/4 height.
    let position = slint::LogicalPosition::new(12.0 + w * 0.75, 52.0 + h * 0.25);
    ui.window()
        .dispatch_event(slint::platform::WindowEvent::PointerMoved { position });
    ui.window()
        .dispatch_event(slint::platform::WindowEvent::PointerScrolled {
            position,
            delta_x: 0.0,
            delta_y: 120.0,
        });
    slint::platform::update_timers_and_animations();
    assert!((ui.get_zoom() - 1.2).abs() < 0.001, "wheel did not zoom");
    assert!(
        (ui.get_pan_x() + w * 0.05).abs() < 0.6,
        "zoom moved cursor point horizontally: pan={}, expected={}",
        ui.get_pan_x(),
        -w * 0.05
    );
    assert!(
        (ui.get_pan_y() - h * 0.05).abs() < 0.6,
        "zoom moved cursor point vertically: pan={}, expected={}",
        ui.get_pan_y(),
        h * 0.05
    );
    // Reverse at the same cursor must return exactly to the original transformation.
    ui.window()
        .dispatch_event(slint::platform::WindowEvent::PointerScrolled {
            position,
            delta_x: 0.0,
            delta_y: -120.0,
        });
    slint::platform::update_timers_and_animations();
    assert!((ui.get_zoom() - 1.0).abs() < 0.001);
    assert!(ui.get_pan_x().abs() < 0.6 && ui.get_pan_y().abs() < 0.6);
    ui.hide().unwrap();
}
