//! Video thumbnails via GStreamer: decode one frame at 10 % of the clip into
//! an appsink, entirely in memory. Codec availability follows the system's
//! (or the Flatpak runtime's) GStreamer plugins.

use std::path::Path;
use std::sync::OnceLock;
use std::time::Duration;

use gst::prelude::*;

use super::{RawImage, Thumb};

fn init() -> Result<(), String> {
    static INIT: OnceLock<Result<(), String>> = OnceLock::new();
    INIT.get_or_init(|| gst::init().map_err(|e| e.to_string())).clone()
}

/// Whether GStreamer could be initialised (videos are hidden otherwise).
pub fn available() -> bool {
    init().is_ok()
}

pub fn thumbnail(path: &Path, size: i32) -> Result<Thumb, String> {
    init()?;
    let uri = gtk::prelude::FileExt::uri(&gtk::gio::File::for_path(path));
    let pipeline = gst::parse::launch(&format!(
        "uridecodebin uri=\"{}\" ! videoconvert ! videoscale ! \
         video/x-raw,format=RGBA,pixel-aspect-ratio=1/1 ! appsink name=sink sync=false max-buffers=1 drop=true",
        uri.replace('"', "%22")
    ))
    .map_err(|e| e.to_string())?
    .downcast::<gst::Pipeline>()
    .map_err(|_| "not a pipeline".to_string())?;
    let result = grab_frame(&pipeline, size);
    let _ = pipeline.set_state(gst::State::Null);
    result
}

fn wait_for_preroll(pipeline: &gst::Pipeline) -> Result<(), String> {
    let bus = pipeline.bus().ok_or("no bus")?;
    let deadline = std::time::Instant::now() + Duration::from_secs(8);
    while std::time::Instant::now() < deadline {
        let Some(msg) = bus.timed_pop(gst::ClockTime::from_mseconds(200)) else { continue };
        match msg.view() {
            gst::MessageView::AsyncDone(_) => return Ok(()),
            gst::MessageView::Error(e) => return Err(e.error().to_string()),
            gst::MessageView::Eos(_) => return Err("end of stream".into()),
            _ => {}
        }
    }
    Err("timed out".into())
}

fn grab_frame(pipeline: &gst::Pipeline, size: i32) -> Result<Thumb, String> {
    pipeline.set_state(gst::State::Paused).map_err(|e| e.to_string())?;
    wait_for_preroll(pipeline)?;
    if let Some(dur) = pipeline.query_duration::<gst::ClockTime>() {
        let target = dur / 10;
        if pipeline.seek_simple(gst::SeekFlags::FLUSH | gst::SeekFlags::KEY_UNIT, target).is_ok() {
            wait_for_preroll(pipeline)?;
        }
    }
    let sink = pipeline.by_name("sink").and_then(|s| s.downcast::<gst_app::AppSink>().ok()).ok_or("no appsink")?;
    let sample = sink.try_pull_preroll(gst::ClockTime::from_seconds(5)).ok_or("no frame")?;
    let caps = sample.caps().ok_or("no caps")?;
    let info = gst_video::VideoInfo::from_caps(caps).map_err(|e| e.to_string())?;
    let buffer = sample.buffer().ok_or("no buffer")?;
    let map = buffer.map_readable().map_err(|e| e.to_string())?;
    let (w, h) = (info.width(), info.height());
    let stride = info.stride()[0] as usize;
    let mut rgba = Vec::with_capacity((w * h * 4) as usize);
    for row in 0..h as usize {
        let start = row * stride;
        rgba.extend_from_slice(&map[start..start + w as usize * 4]);
    }
    let img = image::RgbaImage::from_raw(w, h, rgba).ok_or("bad frame")?;
    let img = image::DynamicImage::ImageRgba8(img);
    let size = size as u32;
    let img = if w > size || h > size { img.thumbnail(size, size) } else { img };
    Ok(Thumb { image: RawImage::from_dynamic(img), width: w, height: h })
}
