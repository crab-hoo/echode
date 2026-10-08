//! C ABI used by the Swift renderer. The scene pointer is owned by the caller.

use crate::{Instance, Scene, View};

fn view_from_parts(
    origin_x: f32,
    origin_y: f32,
    width: f32,
    height: f32,
    zoom_x: f32,
    zoom_y: f32,
) -> View {
    View {
        origin_x,
        origin_y,
        width,
        height,
        zoom_x,
        zoom_y,
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn echode_scene_new() -> *mut Scene {
    Box::into_raw(Box::new(Scene::generate()))
}

#[unsafe(no_mangle)]
pub extern "C" fn echode_scene_free(scene: *mut Scene) {
    if scene.is_null() {
        return;
    }
    // SAFETY: `scene` was returned by `echode_scene_new` and has not been freed.
    drop(unsafe { Box::from_raw(scene) });
}

#[unsafe(no_mangle)]
pub extern "C" fn echode_scene_track_count(scene: *const Scene) -> u32 {
    scene_ref(scene).map_or(0, Scene::track_count)
}

#[unsafe(no_mangle)]
pub extern "C" fn echode_scene_note_count(scene: *const Scene) -> u32 {
    scene_ref(scene).map_or(0, Scene::note_count)
}

#[unsafe(no_mangle)]
pub extern "C" fn echode_scene_timeline_beats(scene: *const Scene) -> f32 {
    scene_ref(scene).map_or(0.0, Scene::timeline_beats)
}

#[unsafe(no_mangle)]
pub extern "C" fn echode_scene_visible_tracks(
    scene: *const Scene,
    origin_x: f32,
    origin_y: f32,
    width: f32,
    height: f32,
    zoom_x: f32,
    zoom_y: f32,
    out: *mut Instance,
    capacity: u32,
    dropped: *mut u32,
) -> u32 {
    write_instances(
        scene,
        view_from_parts(origin_x, origin_y, width, height, zoom_x, zoom_y),
        out,
        capacity,
        dropped,
        Scene::visible_tracks,
    )
}

#[unsafe(no_mangle)]
pub extern "C" fn echode_scene_visible_notes(
    scene: *const Scene,
    origin_x: f32,
    origin_y: f32,
    width: f32,
    height: f32,
    zoom_x: f32,
    zoom_y: f32,
    out: *mut Instance,
    capacity: u32,
    dropped: *mut u32,
) -> u32 {
    write_instances(
        scene,
        view_from_parts(origin_x, origin_y, width, height, zoom_x, zoom_y),
        out,
        capacity,
        dropped,
        Scene::visible_notes,
    )
}

fn scene_ref<'a>(scene: *const Scene) -> Option<&'a Scene> {
    if scene.is_null() {
        None
    } else {
        // SAFETY: the pointer is live until `echode_scene_free`. Callers do not
        // mutate the scene, and they do not call this concurrently with free.
        Some(unsafe { &*scene })
    }
}

fn write_instances(
    scene: *const Scene,
    view: View,
    out: *mut Instance,
    capacity: u32,
    dropped: *mut u32,
    visit: fn(&Scene, View, &mut [Instance]) -> (u32, u32),
) -> u32 {
    let Some(scene) = scene_ref(scene) else {
        return 0;
    };
    if out.is_null() || capacity == 0 {
        return 0;
    }
    // SAFETY: `out` points at `capacity` instances for the duration of this call.
    let buffer = unsafe { std::slice::from_raw_parts_mut(out, capacity as usize) };
    let (written, overflow) = visit(scene, view, buffer);
    if !dropped.is_null() {
        // SAFETY: the caller passed a writable `u32` or null, which was checked.
        unsafe { *dropped = overflow };
    }
    written
}
