use std::{
    cell::RefCell,
    io::{BufRead, BufReader, Write},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, OnceLock,
    },
};

use glib::clone;
use gst::{ClockTime, PadProbeData, PadProbeType, SeekFlags};
use gstreamer_pbutils::{Discoverer, ElementProperties, ElementPropertiesMapItem};
use gtk::{gdk, gio, glib, subclass::prelude::*};

use ges::prelude::*;
use ges::Effect;

use crate::{
    adjustments::{ColorAdjustments, PlaybackSpeed, Repeat},
    info::{get_info, Dimensions, Framerate},
    orientation::VideoOrientation,
    profiles::{ContainerFormat, OutputFormat, Quality, VideoEncoding},
    segments::{ClipRange, SegmentExportMode},
};

mod imp {

    use std::cell::Cell;

    use crate::{orientation::VideoOrientation, widgets::crop::Crop};

    use super::*;

    use adw::subclass::prelude::BinImpl;
    use glib::subclass::Signal;
    use gst::bus::BusWatchGuard;
    use gtk::CompositeTemplate;

    #[derive(CompositeTemplate, Default)]
    #[template(resource = "/io/gitlab/adhami3310/Clips/blueprints/video-preview.ui")]
    pub struct VideoPreview {
        #[template_child]
        pub paint: TemplateChild<gtk::Picture>,
        #[template_child]
        pub crop_box: TemplateChild<Crop>,

        pub current_dimensions: Cell<Option<Dimensions<u32>>>,
        pub orientation: Cell<VideoOrientation>,
        pub audio_level: RefCell<Option<Effect>>,
        /// Live colour correction, mirrored by the `videobalance`/`gamma` effects
        /// below and baked into the export.
        pub adjustments: Cell<ColorAdjustments>,
        pub balance: RefCell<Option<Effect>>,
        pub gamma: RefCell<Option<Effect>>,
        pub sharpness: RefCell<Option<Effect>>,
        pub playback_rate: Cell<f64>,
        pub inpoint: Cell<u64>,
        pub mute: Cell<bool>,
        pub outpoint: Cell<u64>,
        pub effects: RefCell<Vec<String>>,
        pub pipeline: RefCell<Option<ges::Pipeline>>,
        pub clip: RefCell<Option<ges::UriClip>>,
        pub path: RefCell<PathBuf>,
        pub ended: Cell<bool>,
        pub bus_watch: RefCell<Option<BusWatchGuard>>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for VideoPreview {
        const NAME: &'static str = "VideoPreview";
        type Type = super::VideoPreview;
        type ParentType = adw::Bin;

        fn class_init(klass: &mut Self::Class) {
            Self::bind_template(klass);
        }

        fn instance_init(obj: &glib::subclass::InitializingObject<Self>) {
            obj.init_template();
        }

        fn new() -> Self {
            Self::default()
        }
    }

    impl ObjectImpl for VideoPreview {
        fn constructed(&self) {}

        fn signals() -> &'static [Signal] {
            use once_cell::sync::Lazy;
            static SIGNALS: Lazy<[Signal; 4]> = Lazy::new(|| {
                [
                    Signal::builder("orientation-flipped")
                        .param_types(std::iter::empty::<glib::Type>())
                        .build(),
                    Signal::builder("set-position")
                        .param_types([glib::Type::U64])
                        .build(),
                    Signal::builder("preview-ready")
                        .param_types(std::iter::empty::<glib::Type>())
                        .build(),
                    Signal::builder("mode-changed")
                        .param_types([glib::Type::BOOL])
                        .build(),
                ]
            });

            SIGNALS.as_ref()
        }
    }

    impl WidgetImpl for VideoPreview {}

    impl BinImpl for VideoPreview {}
}

glib::wrapper! {
pub struct VideoPreview(ObjectSubclass<imp::VideoPreview>)
    @extends adw::Bin, gtk::Widget,
    @implements gio::ActionMap, gio::ActionGroup, gtk::Root, gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl Default for VideoPreview {
    fn default() -> Self {
        Self::new()
    }
}

#[gtk::template_callbacks]
impl VideoPreview {
    pub fn new() -> Self {
        let bin = glib::Object::builder::<VideoPreview>().build();

        bin
    }

    pub fn reset(&self) {
        self.imp().crop_box.reset();
        self.imp().orientation.set(VideoOrientation::Identity);
        self.imp().audio_level.replace(None);
        self.imp().adjustments.set(ColorAdjustments::NEUTRAL);
        self.imp().balance.replace(None);
        self.imp().gamma.replace(None);
        self.imp().sharpness.replace(None);
        self.imp().playback_rate.set(1.0);
        self.imp().effects.replace(vec![]);
        self.imp().current_dimensions.set(None);
        {
            if let Some(pipeline) = self.imp().pipeline.take() {
                pipeline.set_state(gst::State::Null).unwrap();
            }
        }
        self.imp().clip.replace(None);
        self.imp().path.replace(PathBuf::new());
        self.imp().ended.replace(false);
        self.imp().bus_watch.replace(None);
        self.imp().paint.set_paintable(None::<&gdk::Paintable>);
        self.imp().mute.set(false);
        self.emit_by_name::<()>("mode-changed", &[&false]);
    }

    async fn load_ges_clip(&self, uri: &str) -> Result<ges::Asset, ()> {
        let clip = ges::Asset::request_future(ges::UriClip::static_type(), Some(uri))
            .await
            .map_err(|_| ())?;

        Ok(clip)
    }

    pub async fn load_path(
        &self,
        path: PathBuf,
    ) -> Result<(Dimensions<u32>, u64, Option<Framerate>, bool), ()> {
        dbg!(url::Url::from_file_path(path.clone()).unwrap().as_str());

        let clip = self
            .load_ges_clip(url::Url::from_file_path(path.clone()).unwrap().as_str())
            .await
            .map_err(|_| ())?
            .extract()
            .unwrap()
            .dynamic_cast::<ges::UriClip>()
            .unwrap();

        let duration = clip.duration().mseconds();
        self.imp().clip.replace(Some(clip));

        self.imp().inpoint.set(0);
        self.imp().outpoint.set(duration);

        let (dimensions, framerate, has_audio) =
            get_info(path.to_str().unwrap().to_owned()).ok_or(())?;

        self.imp().current_dimensions.set(Some(dimensions));

        self.imp().path.replace(path);
        self.imp().effects.replace(vec![]);

        self.imp().crop_box.set_proportions((0., 0., 0., 0.));
        self.imp().audio_level.replace(None);
        // The colour effects belonged to the previous clip; they get re-created
        // lazily on the new one the first time an adjustment is made.
        self.imp().adjustments.set(ColorAdjustments::NEUTRAL);
        self.imp().balance.replace(None);
        self.imp().gamma.replace(None);
        self.imp().sharpness.replace(None);
        self.imp().orientation.set(Default::default());
        self.emit_by_name::<()>("mode-changed", &[&false]);

        if has_audio {
            self.imp().mute.set(false);
        } else {
            self.imp().mute.set(true);
        }

        self.refresh_ui();

        Ok((dimensions, duration, framerate, has_audio))
    }

    pub fn seek(&self, position: u64) {
        self.pause();

        self.quiet_seek(position);
    }

    pub fn quiet_seek(&self, position: u64) {
        if position == self.imp().outpoint.get() {
            self.imp().ended.set(true);
        }

        let position = position.max(self.imp().inpoint.get()) - self.imp().inpoint.get();
        self.seek_timeline_position(position);
    }

    fn playback_rate(&self) -> f64 {
        let rate = self.imp().playback_rate.get();
        if rate > 0.0 {
            rate
        } else {
            1.0
        }
    }

    fn seek_timeline_position(&self, position_ms: u64) {
        if let Some(pipeline) = self.imp().pipeline.borrow().as_ref() {
            if let Err(err) = pipeline.seek(
                self.playback_rate(),
                SeekFlags::FLUSH | SeekFlags::ACCURATE,
                gst::SeekType::Set,
                ClockTime::from_mseconds(position_ms),
                gst::SeekType::None,
                ClockTime::NONE,
            ) {
                log::warn!("could not apply preview playback rate: {err}");
            }
        }
    }

    /// Changes preview speed while keeping the current source position.
    pub fn set_playback_rate(&self, rate: f64) {
        let rate = rate.clamp(0.25, 1.0);
        if (rate - self.playback_rate()).abs() < 0.0005 {
            return;
        }

        let position = self
            .imp()
            .pipeline
            .borrow()
            .as_ref()
            .and_then(|pipeline| pipeline.query_position::<ClockTime>())
            .map(ClockTime::mseconds);
        self.imp().playback_rate.set(rate);
        if let Some(position) = position {
            self.seek_timeline_position(position);
        }
    }

    pub fn refresh_ui(&self) {
        self.kill();

        let timeline = ges::Timeline::new_audio_video();
        if let Some(t) = timeline.tracks().first() {
            t.set_restriction_caps(
                &gst::Caps::builder("video/x-raw")
                    .field("framerate", gst::Fraction::new(30, 1))
                    .build(),
            );
        }

        let original_clip = self.imp().clip.borrow();
        let clip = original_clip.as_ref().unwrap();

        let layer = timeline.append_layer();
        layer.add_clip(clip).unwrap();

        let pipeline = ges::Pipeline::new();
        pipeline.set_timeline(&timeline).unwrap();

        let gtksink = gst::ElementFactory::make("gtk4paintablesink")
            .build()
            .unwrap();

        let paintable = gtksink.property::<gdk::Paintable>("paintable");

        self.imp().paint.set_paintable(Some(&paintable));

        let sink = gst::Bin::default();
        let convert = gst::ElementFactory::make("videoconvertscale")
            .build()
            .unwrap();

        sink.add(&convert).unwrap();
        sink.add(&gtksink).unwrap();
        convert.link(&gtksink).unwrap();

        let pad = &gst::GhostPad::with_target(&convert.static_pad("sink").unwrap()).unwrap();

        let (sender, receiver) = async_channel::bounded(1);

        pad.add_probe(PadProbeType::DATA_DOWNSTREAM, move |_, info| {
            if let Some(PadProbeData::Buffer(data)) = &info.data {
                if let Some(pts) = data.pts() {
                    sender
                        .send_blocking(pts.mseconds())
                        .expect("Concurrency Issues");
                }
            }
            gst::PadProbeReturn::Ok
        });

        sink.add_pad(pad).unwrap();

        pipeline.set_video_sink(Some(&sink));

        let bus = pipeline.bus().unwrap();

        pipeline
            .set_state(gst::State::Paused)
            .expect("Unable to set the pipeline to the `Paused` state");

        let bus_watch = bus
            .add_watch_local(clone!(
                #[weak(rename_to=this)]
                self,
                #[upgrade_or]
                glib::ControlFlow::Break,
                move |_, msg| {
                    use gst::MessageView;

                    match msg.view() {
                        MessageView::Eos(..) => {
                            this.pause();
                            this.imp().ended.set(true);
                            // this.emit_by_name::<()>("set-position", &[&this.imp().inpoint.get()]);
                            // this.seek(0);
                        }
                        MessageView::Error(err) => {
                            println!(
                                "Error from {:?}: {} ({:?})",
                                err.src().map(|s| s.path_string()),
                                err.error(),
                                err.debug()
                            );
                        }
                        _ => (),
                    };

                    glib::ControlFlow::Continue
                }
            ))
            .expect("Failed to add bus watch");

        self.imp().pipeline.replace(Some(pipeline));
        self.imp().bus_watch.replace(Some(bus_watch));
        self.seek_timeline_position(0);

        glib::spawn_future_local(clone!(
            #[weak(rename_to = this)]
            self,
            async move {
                let mut sent_ready = false;

                while let Ok(p) = receiver.recv().await {
                    if !sent_ready {
                        sent_ready = true;
                        this.emit_by_name::<()>("preview-ready", &[]);
                    }
                    if this.is_playing() {
                        this.emit_by_name::<()>("set-position", &[&(p + this.imp().inpoint.get())]);
                    }
                }
            }
        ));
    }

    // fn update_position(&self) {
    //     let position = self.imp().pipeline.borrow().as_ref().unwrap().query_position::<gst::format::ClockTime>().unwrap().mseconds();
    //     self.imp().timeline.set_position(position);
    // }

    pub fn pause(&self) {
        let orig_p = self.imp().pipeline.borrow();
        let p = orig_p.as_ref().unwrap();

        p.set_state(gst::State::Paused).unwrap();
        self.emit_by_name::<()>("mode-changed", &[&false]);
        // self.imp().play_pause.set_icon_name("play-symbolic");
    }

    fn is_playing(&self) -> bool {
        let orig_p = self.imp().pipeline.borrow();
        let p = orig_p.as_ref().unwrap();

        matches!(p.current_state(), gst::State::Playing)
    }

    pub fn play(&self) {
        let orig_p = self.imp().pipeline.borrow();
        let p = orig_p.as_ref().unwrap();

        if self.imp().ended.get() {
            self.imp().ended.set(false);
            self.quiet_seek(0);
        }

        p.set_state(gst::State::Playing).unwrap();
        self.emit_by_name::<()>("mode-changed", &[&true]);
        // self.imp().play_pause.set_icon_name("pause-symbolic");
    }

    pub fn set_range(&self, start: u64, end: u64) {
        let original_clip = self.imp().clip.borrow();
        let clip = original_clip.as_ref();
        if let Some(clip) = clip {
            clip.set_inpoint(ClockTime::from_mseconds(start));
            clip.set_duration(Some(ClockTime::from_mseconds(end - start)));
            self.imp().inpoint.set(start);
            self.imp().outpoint.set(end);
        }
        self.commit();
    }

    fn add_effect(&self, effect: &ges::Effect) {
        let original_clip = self.imp().clip.borrow();
        let clip = original_clip.as_ref();

        if let Some(clip) = clip {
            clip.add_top_effect(effect, 0).unwrap();
        }
        self.commit();
    }

    fn commit(&self) {
        let orig_p = self.imp().pipeline.borrow();
        let p = orig_p.as_ref().unwrap();

        p.timeline().unwrap().commit_sync();
    }

    fn remove_effect(&self, effect: &ges::Effect) {
        let original_clip = self.imp().clip.borrow();
        let clip = original_clip.as_ref();

        if let Some(clip) = clip {
            clip.remove(effect).unwrap();
        }
    }

    pub fn rotate_right(&self) {
        self.imp()
            .orientation
            .set(self.imp().orientation.get().rotate_right());
        self.add_effect(&ges::Effect::new("videoflip method=clockwise").unwrap());
        self.imp()
            .effects
            .borrow_mut()
            .push("videoflip method=clockwise".to_owned());
        self.emit_by_name::<()>("orientation-flipped", &[]);
        self.imp()
            .crop_box
            .set_proportions(self.imp().crop_box.rotate_right_proportions());
        let dimensions = self.imp().current_dimensions.get().unwrap();
        self.imp().current_dimensions.set(Some(dimensions.swap()));
    }

    pub fn rotate_left(&self) {
        self.imp()
            .orientation
            .set(self.imp().orientation.get().rotate_left());
        self.add_effect(&ges::Effect::new("videoflip method=counterclockwise").unwrap());
        self.imp()
            .effects
            .borrow_mut()
            .push("videoflip method=counterclockwise".to_owned());
        self.emit_by_name::<()>("orientation-flipped", &[]);
        self.imp()
            .crop_box
            .set_proportions(self.imp().crop_box.rotate_left_proportions());

        let dimensions = self.imp().current_dimensions.get().unwrap();
        self.imp().current_dimensions.set(Some(dimensions.swap()));
    }

    pub fn horizontal_flip(&self) {
        self.imp()
            .orientation
            .set(self.imp().orientation.get().horizontal_flip());
        self.add_effect(&ges::Effect::new("videoflip method=horizontal-flip").unwrap());
        self.imp()
            .effects
            .borrow_mut()
            .push("videoflip method=horizontal-flip".to_owned());

        self.imp()
            .crop_box
            .set_proportions(self.imp().crop_box.horizontal_flip_proportions());
    }

    pub fn vertical_flip(&self) {
        // self.replace_with_thumbnail();
        self.imp()
            .orientation
            .set(self.imp().orientation.get().vertical_flip());
        self.add_effect(&ges::Effect::new("videoflip method=vertical-flip").unwrap());
        self.imp()
            .effects
            .borrow_mut()
            .push("videoflip method=vertical-flip".to_owned());

        self.imp()
            .crop_box
            .set_proportions(self.imp().crop_box.vertical_flip_proportions());
    }

    /// Applies colour correction to the live preview. The `videobalance`/`gamma`
    /// effects are attached to the clip on first use and then only have their
    /// properties updated, so dragging a slider does not rebuild the pipeline.
    pub fn set_color_adjustments(&self, adjustments: ColorAdjustments) {
        self.imp().adjustments.set(adjustments);

        // Nothing to drive yet, or nothing to do: don't pay for the effects while
        // every knob is still neutral.
        if self.imp().clip.borrow().is_none() || self.imp().pipeline.borrow().is_none() {
            return;
        }
        if adjustments.is_neutral() && self.imp().balance.borrow().is_none() {
            return;
        }

        self.ensure_adjustment_effects();

        if let Some(balance) = self.imp().balance.borrow().as_ref() {
            set_child_property_f64(balance, "brightness", adjustments.brightness);
            set_child_property_f64(balance, "contrast", adjustments.contrast);
            set_child_property_f64(balance, "saturation", adjustments.saturation);
            set_child_property_f64(balance, "hue", adjustments.gst_hue());
        }
        if let Some(gamma) = self.imp().gamma.borrow().as_ref() {
            set_child_property_f64(gamma, "gamma", adjustments.gamma);
        }
        if let Some(sharpness) = self.imp().sharpness.borrow().as_ref() {
            set_child_property_f64(sharpness, "sigma", adjustments.gst_sharpen_sigma());
        }

        self.commit();
    }

    /// The colour correction currently shown in the preview.
    pub fn color_adjustments(&self) -> ColorAdjustments {
        self.imp().adjustments.get()
    }

    fn ensure_adjustment_effects(&self) {
        if self.imp().balance.borrow().is_some() {
            return;
        }

        let Ok(balance) = ges::Effect::new("videobalance") else {
            log::warn!("videobalance is unavailable; colour adjustments will not preview");
            return;
        };
        let Ok(gamma) = ges::Effect::new("gamma") else {
            log::warn!("gamma is unavailable; colour adjustments will not preview");
            return;
        };
        let sharpness = ges::Effect::new(ColorAdjustments::GST_SHARPEN_EFFECT).ok();
        if sharpness.is_none() {
            log::warn!("gaussianblur is unavailable; sharpness will not preview");
        }

        if let Some(clip) = self.imp().clip.borrow().as_ref() {
            clip.add_top_effect(&balance, 0).ok();
            clip.add_top_effect(&gamma, 0).ok();
            if let Some(sharpness) = sharpness.as_ref() {
                clip.add_top_effect(sharpness, 0).ok();
            }
        }

        self.imp().balance.replace(Some(balance));
        self.imp().gamma.replace(Some(gamma));
        self.imp().sharpness.replace(sharpness);
    }

    pub fn mute(&self) {
        self.imp().mute.set(true);

        let new_av = ges::Effect::new("volume volume=0").unwrap();
        let av_orig = self.imp().audio_level.replace(Some(new_av));

        if let Some(av_orig) = av_orig {
            self.remove_effect(&av_orig);
        }

        self.add_effect(self.imp().audio_level.borrow().as_ref().unwrap());
    }

    pub fn unmute(&self) {
        self.imp().mute.set(false);

        let new_av = ges::Effect::new("volume volume=1").unwrap();
        let av_orig = self.imp().audio_level.replace(Some(new_av));

        if let Some(av_orig) = av_orig {
            self.remove_effect(&av_orig);
        }

        self.add_effect(self.imp().audio_level.borrow().as_ref().unwrap());
    }

    pub fn kill(&self) {
        if let Some(pipeline) = self.imp().pipeline.borrow_mut().take() {
            pipeline
                .set_state(gst::State::Null)
                .expect("Unable to set the pipeline to the `Null` state");
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub fn save(
        &self,
        output_path: PathBuf,
        sender: async_channel::Sender<Result<(u64, u64), ()>>,
        output_format: OutputFormat,
        framerate: Framerate,
        scaled_dimension: Dimensions<u32>,
        prefer_gpu: bool,
        repeat: Repeat,
        speed: PlaybackSpeed,
        segments: Vec<ClipRange>,
        segment_export_mode: SegmentExportMode,
        running_flag: Arc<AtomicBool>,
    ) {
        self.kill();

        dbg!(&output_format, &framerate, &scaled_dimension, prefer_gpu);

        let adjustments = self.imp().adjustments.get();

        let input_path = self.imp().path.borrow().to_owned();
        let orientation = self.imp().orientation.get();

        let dimensions = get_info(input_path.to_str().unwrap().to_owned()).unwrap().0;

        let dimensions: Dimensions<f64> = dimensions.into();

        let dimensions = match orientation.is_width_height_swapped() {
            false => dimensions,
            true => dimensions.swap(),
        };

        let (top, right, bottom, left) = self.imp().crop_box.proportions();

        let full_scaled_width = dimensions.width
            * (scaled_dimension.width_f64() / (dimensions.width * (1. - right - left)));
        let full_scaled_height = dimensions.height
            * (scaled_dimension.height_f64() / (dimensions.height * (1. - top - bottom)));

        let mute = self.imp().mute.get();

        let inpoint = self.imp().clip.borrow().as_ref().unwrap().inpoint();
        let duration = self.imp().clip.borrow().as_ref().unwrap().duration();

        // Fast path: render with ffmpeg (NVENC + libplacebo). GES stays as fallback
        // for the cases ffmpeg doesn't cover here (GIF, "keep as-is").
        if Self::ffmpeg_can_handle(&output_format) {
            if segments.len() > 1 {
                Self::save_ffmpeg_segments(
                    input_path,
                    output_path,
                    output_format,
                    framerate,
                    dimensions,
                    (top, right, bottom, left),
                    scaled_dimension,
                    orientation,
                    mute,
                    prefer_gpu,
                    adjustments,
                    speed,
                    segments,
                    segment_export_mode,
                    sender,
                    running_flag,
                );
                return;
            }

            let range = segments.first().copied().unwrap_or_else(|| {
                ClipRange::new(inpoint.mseconds(), inpoint.mseconds() + duration.mseconds())
            });
            Self::save_ffmpeg(
                input_path,
                output_path,
                output_format,
                framerate,
                dimensions,
                (top, right, bottom, left),
                scaled_dimension,
                orientation,
                mute,
                range.start_ms * 1_000_000,
                range.duration_ms() * 1_000_000,
                prefer_gpu,
                adjustments,
                repeat,
                speed,
                sender,
                running_flag,
            );
            return;
        }

        if !repeat.is_off() {
            log::warn!(
                "loop/boomerang is only supported by the ffmpeg render path; \
                 exporting the selection once"
            );
        }

        Self::save_ges(
            input_path,
            output_path,
            output_format,
            framerate,
            scaled_dimension,
            full_scaled_width,
            full_scaled_height,
            top,
            left,
            orientation,
            mute,
            inpoint,
            duration,
            prefer_gpu,
            adjustments,
            sender,
            running_flag,
        );
    }

    #[allow(clippy::too_many_arguments)]
    fn save_ges(
        input_path: PathBuf,
        output_path: PathBuf,
        output_format: OutputFormat,
        framerate: Framerate,
        scaled_dimension: Dimensions<u32>,
        full_scaled_width: f64,
        full_scaled_height: f64,
        top: f64,
        left: f64,
        orientation: VideoOrientation,
        mute: bool,
        inpoint: ClockTime,
        duration: ClockTime,
        prefer_gpu: bool,
        adjustments: ColorAdjustments,
        sender: async_channel::Sender<Result<(u64, u64), ()>>,
        running_flag: Arc<AtomicBool>,
    ) {
        std::thread::spawn(move || {
            let clip = ges::UriClip::new(
                url::Url::from_file_path(input_path.clone())
                    .unwrap()
                    .as_str(),
            )
            .unwrap();

            let timeline = ges::Timeline::new_audio_video();

            let layer = timeline.append_layer();
            layer.add_clip(&clip).unwrap();

            if let Some(t) = timeline.tracks().first() {
                t.set_restriction_caps(
                    &gst::Caps::builder("video/x-raw")
                        .field(
                            "framerate",
                            gst::Fraction::new(
                                framerate.nominator as i32,
                                framerate.denominator as i32,
                            ),
                        )
                        .field("width", scaled_dimension.width as i32)
                        .field("height", scaled_dimension.height as i32)
                        .build(),
                );
                t.elements().into_iter().for_each(|te| {
                    ges::prelude::TrackElementExt::set_child_property(
                        &te,
                        "video-direction",
                        &match orientation {
                            VideoOrientation::Identity => {
                                gstreamer_video::VideoOrientationMethod::Identity
                            }
                            VideoOrientation::R90 => gstreamer_video::VideoOrientationMethod::_90r,
                            VideoOrientation::R180 => gstreamer_video::VideoOrientationMethod::_180,
                            VideoOrientation::R270 => gstreamer_video::VideoOrientationMethod::_90l,
                            VideoOrientation::FlippedIdentity => {
                                gstreamer_video::VideoOrientationMethod::Horiz
                            }
                            VideoOrientation::FR180 => {
                                gstreamer_video::VideoOrientationMethod::Vert
                            }
                            VideoOrientation::FR90 => gstreamer_video::VideoOrientationMethod::UrLl,
                            VideoOrientation::FR270 => {
                                gstreamer_video::VideoOrientationMethod::UlLr
                            }
                        }
                        .to_value(),
                    )
                    .unwrap();

                    ges::prelude::TrackElementExt::set_child_property(
                        &te,
                        "width",
                        &(full_scaled_width as i32).to_value(),
                    )
                    .unwrap();
                    ges::prelude::TrackElementExt::set_child_property(
                        &te,
                        "height",
                        &(full_scaled_height as i32).to_value(),
                    )
                    .unwrap();
                    ges::prelude::TrackElementExt::set_child_property(
                        &te,
                        "posx",
                        &((-left * full_scaled_width) as i32).to_value(),
                    )
                    .unwrap();
                    ges::prelude::TrackElementExt::set_child_property(
                        &te,
                        "posy",
                        &((-top * full_scaled_height) as i32).to_value(),
                    )
                    .unwrap();
                });
            }

            clip.add_top_effect(&ges::Effect::new("videorate").unwrap(), 0)
                .ok();

            if !adjustments.is_neutral() {
                if let Ok(balance) = ges::Effect::new("videobalance") {
                    set_child_property_f64(&balance, "brightness", adjustments.brightness);
                    set_child_property_f64(&balance, "contrast", adjustments.contrast);
                    set_child_property_f64(&balance, "saturation", adjustments.saturation);
                    set_child_property_f64(&balance, "hue", adjustments.gst_hue());
                    clip.add_top_effect(&balance, 0).ok();
                }
                if let Ok(gamma) = ges::Effect::new("gamma") {
                    set_child_property_f64(&gamma, "gamma", adjustments.gamma);
                    clip.add_top_effect(&gamma, 0).ok();
                }
                if adjustments.sharpness >= 0.0005 {
                    if let Ok(sharpness) = ges::Effect::new(ColorAdjustments::GST_SHARPEN_EFFECT) {
                        set_child_property_f64(
                            &sharpness,
                            "sigma",
                            adjustments.gst_sharpen_sigma(),
                        );
                        clip.add_top_effect(&sharpness, 0).ok();
                    }
                }
            }

            clip.set_inpoint(inpoint);
            clip.set_duration(Some(duration));

            let pipeline = ges::Pipeline::new();
            pipeline.set_timeline(&timeline).unwrap();

            if output_format.container_format == ContainerFormat::GifContainer {
                pipeline
                    .set_render_settings(
                        url::Url::from_file_path(output_path).unwrap().as_str(),
                        &gstreamer_pbutils::EncodingVideoProfile::builder(
                            &gst::Caps::builder(output_format.video_encoding.unwrap().get_format())
                                .build(),
                        )
                        .preset_name(output_format.video_encoding.unwrap().get_preset_name())
                        .build(),
                    )
                    .unwrap();
            } else if output_format.container_format == ContainerFormat::Same {
                let profile = gstreamer_pbutils::EncodingProfile::from_discoverer(
                    &Discoverer::new(gst::ClockTime::SECOND)
                        .unwrap()
                        .discover_uri(
                            url::Url::from_file_path(input_path.clone())
                                .unwrap()
                                .as_str(),
                        )
                        .unwrap(),
                )
                .unwrap();

                let (video_caps, audio_caps): (Vec<_>, Vec<_>) = profile
                    .input_caps()
                    .iter()
                    .map(|ic| {
                        let mut ic = ic.to_owned();
                        ic.remove_fields(["width", "height", "framerate"]);

                        let mut caps = gst::Caps::builder(ic.name());

                        for (name, value) in ic.into_iter() {
                            caps = caps.field(name, value.clone());
                        }

                        caps.build()
                    })
                    .partition(|c| c.to_string().starts_with("video"));

                let profile_format = profile.format();

                let mut container_profile =
                    gstreamer_pbutils::EncodingContainerProfile::builder(&profile_format)
                        .name("container");

                if let Some(video_cap) = video_caps.first() {
                    let video_profile =
                        gstreamer_pbutils::EncodingVideoProfile::builder(video_cap).build();

                    container_profile = container_profile.add_profile(video_profile);
                }

                if !mute {
                    if let Some(audio_cap) = audio_caps.first() {
                        let audio_profile =
                            gstreamer_pbutils::EncodingAudioProfile::builder(audio_cap).build();

                        container_profile = container_profile.add_profile(audio_profile);
                    }
                }

                pipeline
                    .set_render_settings(
                        url::Url::from_file_path(output_path).unwrap().as_str(),
                        &container_profile.build(),
                    )
                    .unwrap();
            } else {
                let video_encoding = output_format.video_encoding.unwrap();
                let encoder = video_encoding.resolve_encoder(prefer_gpu);
                log::info!(
                    "Export encoder: {} [{}]",
                    encoder.element,
                    if encoder.hardware {
                        "GPU/NVENC"
                    } else {
                        "CPU/software"
                    }
                );
                let video_caps = gst::Caps::builder(video_encoding.get_format()).build();

                let video_profile_builder =
                    gstreamer_pbutils::EncodingVideoProfile::builder(&video_caps)
                        .preset_name(encoder.element);

                let fps = framerate.nominator as f64 / (framerate.denominator.max(1) as f64);
                let target_kbps = output_format.quality.target_kbps(
                    scaled_dimension.width,
                    scaled_dimension.height,
                    fps,
                );
                let video_profile = if let Some(bitrate_kbps) = target_kbps {
                    let (prop_name, multiplier) = encoder.bitrate_property;
                    if prop_name.is_empty() {
                        video_profile_builder.build()
                    } else {
                        let bitrate_value = bitrate_kbps as i32 * multiplier as i32;
                        let props = ElementProperties::builder_map()
                            .item(
                                ElementPropertiesMapItem::builder(encoder.element)
                                    .field(prop_name, bitrate_value)
                                    .build(),
                            )
                            .build();
                        video_profile_builder.element_properties(props).build()
                    }
                } else {
                    video_profile_builder.build()
                };

                let container_format =
                    gst::Caps::builder(output_format.container_format.format()).build();

                let mut container_profile =
                    gstreamer_pbutils::EncodingContainerProfile::builder(&container_format)
                        .name("container")
                        .add_profile(video_profile);

                if !mute {
                    let audio_profile = gstreamer_pbutils::EncodingAudioProfile::builder(
                        &gst::Caps::builder(output_format.audio_encoding.unwrap().get_format())
                            .build(),
                    )
                    .build();
                    container_profile = container_profile.add_profile(audio_profile);
                }

                pipeline
                    .set_render_settings(
                        url::Url::from_file_path(output_path).unwrap().as_str(),
                        &container_profile.build(),
                    )
                    .unwrap();
            }

            pipeline.set_mode(ges::PipelineFlags::RENDER).unwrap();

            let sender_pad = sender.clone();

            let another_running_flag = running_flag.clone();

            timeline.pads().first().unwrap().add_probe(
                PadProbeType::DATA_DOWNSTREAM,
                move |_, info| {
                    if let Some(PadProbeData::Buffer(data)) = &info.data {
                        if let Some(pts) = data.pts() {
                            if sender_pad
                                .send_blocking(Ok((pts.mseconds(), duration.mseconds())))
                                .is_err()
                            {
                                return gst::PadProbeReturn::Drop;
                            }
                        }
                    }

                    if !another_running_flag
                        .clone()
                        .load(std::sync::atomic::Ordering::SeqCst)
                    {
                        sender_pad
                            .send_blocking(Err(()))
                            .expect("Concurrency Issues");
                        return gst::PadProbeReturn::Drop;
                    }

                    gst::PadProbeReturn::Ok
                },
            );

            pipeline.set_state(gst::State::Playing).unwrap();

            let bus = pipeline
                .bus()
                .expect("Pipeline without bus. Shouldn't happen!");

            dbg!("starting");

            for msg in bus.iter_timed(gst::ClockTime::NONE) {
                use gst::MessageView;

                match msg.view() {
                    MessageView::Eos(..) => {
                        sender
                            .send_blocking(Ok((1, 1)))
                            .expect("Concurrency Issues");
                        break;
                    }
                    MessageView::Error(_) => {
                        pipeline.set_state(gst::State::Null).unwrap();

                        sender.send_blocking(Err(())).expect("Concurrency Issues");
                    }
                    _ => {
                        if !running_flag.load(std::sync::atomic::Ordering::SeqCst) {
                            pipeline.set_state(gst::State::Null).unwrap();

                            sender.send_blocking(Err(())).expect("Concurrency Issues");
                        }
                    }
                }
            }

            pipeline.set_state(gst::State::Null).unwrap();
        });
    }

    /// Whether the ffmpeg render path can handle this format. GIF and "keep as-is"
    /// fall back to the GES path.
    fn ffmpeg_can_handle(output_format: &OutputFormat) -> bool {
        !matches!(
            output_format.container_format,
            ContainerFormat::Same | ContainerFormat::GifContainer
        ) && !matches!(
            output_format.video_encoding,
            Some(VideoEncoding::Gif) | None
        )
    }

    /// Renders each source range serially. Joined exports concatenate the encoded
    /// parts by stream copy; separate exports write one safely named file per range.
    #[allow(clippy::too_many_arguments)]
    fn save_ffmpeg_segments(
        input_path: PathBuf,
        output_target: PathBuf,
        output_format: OutputFormat,
        framerate: Framerate,
        oriented: Dimensions<f64>,
        crop: (f64, f64, f64, f64),
        scaled: Dimensions<u32>,
        orientation: VideoOrientation,
        mute: bool,
        prefer_gpu: bool,
        adjustments: ColorAdjustments,
        speed: PlaybackSpeed,
        segments: Vec<ClipRange>,
        export_mode: SegmentExportMode,
        sender: async_channel::Sender<Result<(u64, u64), ()>>,
        running_flag: Arc<AtomicBool>,
    ) {
        std::thread::spawn(move || {
            let segments = segments
                .into_iter()
                .filter(|range| range.duration_ms() > 0)
                .collect::<Vec<_>>();
            if segments.is_empty() {
                let _ = sender.send_blocking(Err(()));
                return;
            }

            let filters = ffmpeg_filter_chain(
                oriented,
                crop,
                scaled,
                orientation,
                adjustments,
                speed,
                framerate,
            );
            let video_encoding = output_format.video_encoding.unwrap();
            let (software, hardware) = video_encoding.ffmpeg_encoders();
            let encoder = if prefer_gpu {
                hardware
                    .filter(|name| ffmpeg_has_encoder(name))
                    .unwrap_or(software)
            } else {
                software
            };
            let quality_args = ffmpeg_quality_args(encoder, output_format.quality);
            let audio_codec = if mute {
                None
            } else {
                output_format
                    .audio_encoding
                    .map(|audio| audio.ffmpeg_codec())
            };
            let ext = output_format.container_format.extension().to_owned();
            let source_ms = speed.output_duration_ms(
                segments
                    .iter()
                    .map(|range| range.duration_ms())
                    .sum::<u64>(),
            );
            let concat_ms = if export_mode == SegmentExportMode::Join {
                source_ms / 10 + 1
            } else {
                0
            };
            let total_ms = source_ms + concat_ms;
            let mut done_ms = 0;
            let mut temps = TempFiles::default();
            let mut rendered_parts = Vec::with_capacity(segments.len());

            log::info!(
                "exporting {} sections as {:?} with {}",
                segments.len(),
                export_mode,
                encoder
            );

            for (index, range) in segments.iter().enumerate() {
                let part_path = match export_mode {
                    SegmentExportMode::Join => {
                        temps.reserve(&output_target, &format!("section-{index}"), &ext)
                    }
                    SegmentExportMode::Separate => {
                        unique_segment_path(&output_target, &input_path, index, &ext)
                    }
                };

                let mut cmd = ffmpeg_base();
                cmd.args([
                    "-ss",
                    &format!("{:.3}", range.start_ms as f64 / 1000.0),
                    "-t",
                    &format!("{:.3}", range.duration_ms() as f64 / 1000.0),
                ]);
                cmd.arg("-i").arg(&input_path);
                cmd.args(["-vf", &filters.join(",")]);
                cmd.arg("-c:v").arg(encoder);
                cmd.args(&quality_args);
                match audio_codec {
                    Some(codec) => {
                        if let Some(filter) = speed.ffmpeg_audio_filter() {
                            cmd.args(["-af", filter]);
                        }
                        cmd.arg("-c:a").arg(codec);
                    }
                    None => {
                        cmd.arg("-an");
                    }
                }
                cmd.arg(&part_path);

                let span = speed.output_duration_ms(range.duration_ms());
                if run_ffmpeg_stage(
                    &mut cmd,
                    span,
                    done_ms,
                    span,
                    total_ms,
                    &sender,
                    &running_flag,
                ) != StageOutcome::Done
                {
                    let _ = sender.send_blocking(Err(()));
                    return;
                }
                done_ms += span;
                rendered_parts.push(part_path);
            }

            if export_mode == SegmentExportMode::Separate {
                let _ = sender.send_blocking(Ok((total_ms, total_ms)));
                return;
            }

            let list_path = temps.reserve(&output_target, "sections", "txt");
            let list = rendered_parts
                .iter()
                .map(|part| concat_entry(part))
                .collect::<String>();
            if let Err(err) = std::fs::File::create(&list_path)
                .and_then(|mut file| file.write_all(list.as_bytes()))
            {
                log::error!("could not write the section list: {err}");
                let _ = sender.send_blocking(Err(()));
                return;
            }

            let mut cmd = ffmpeg_base();
            cmd.args(["-f", "concat", "-safe", "0"]);
            cmd.arg("-i").arg(&list_path);
            cmd.args(["-c", "copy"]);
            cmd.arg(&output_target);
            let mut outcome = run_ffmpeg_stage(
                &mut cmd,
                source_ms,
                done_ms,
                concat_ms,
                total_ms,
                &sender,
                &running_flag,
            );

            if outcome == StageOutcome::Failed {
                log::warn!("section stream-copy concat failed; re-encoding the joined result");
                let mut cmd = ffmpeg_base();
                cmd.args(["-f", "concat", "-safe", "0"]);
                cmd.arg("-i").arg(&list_path);
                cmd.arg("-c:v").arg(encoder);
                cmd.args(&quality_args);
                match audio_codec {
                    Some(codec) => {
                        cmd.arg("-c:a").arg(codec);
                    }
                    None => {
                        cmd.arg("-an");
                    }
                }
                cmd.arg(&output_target);
                outcome = run_ffmpeg_stage(
                    &mut cmd,
                    source_ms,
                    done_ms,
                    concat_ms,
                    total_ms,
                    &sender,
                    &running_flag,
                );
            }

            let _ = sender.send_blocking(if outcome == StageOutcome::Done {
                Ok((total_ms, total_ms))
            } else {
                Err(())
            });
        });
    }

    /// Render via ffmpeg subprocesses: trim -> orient -> crop -> colour -> scale
    /// (libplacebo) -> fps -> NVENC/software encode. Loop and boomerang add a reversed
    /// pass and a stream-copy concat on top. Reports progress and supports cancellation
    /// through the same channel/flag protocol as the GES path.
    #[allow(clippy::too_many_arguments)]
    fn save_ffmpeg(
        input_path: PathBuf,
        output_path: PathBuf,
        output_format: OutputFormat,
        framerate: Framerate,
        oriented: Dimensions<f64>,
        crop: (f64, f64, f64, f64),
        scaled: Dimensions<u32>,
        orientation: VideoOrientation,
        mute: bool,
        inpoint_ns: u64,
        duration_ns: u64,
        prefer_gpu: bool,
        adjustments: ColorAdjustments,
        repeat: Repeat,
        speed: PlaybackSpeed,
        sender: async_channel::Sender<Result<(u64, u64), ()>>,
        running_flag: Arc<AtomicBool>,
    ) {
        std::thread::spawn(move || {
            let (top, right, bottom, left) = crop;

            // Filter chain: orient -> crop (in oriented space) -> colour -> scale -> fps.
            let mut filters: Vec<String> = orientation
                .ffmpeg_filters()
                .iter()
                .map(|f| f.to_string())
                .collect();

            let src_w = oriented.width;
            let src_h = oriented.height;
            let crop_w = (((1.0 - left - right) * src_w).round() as i64 / 2 * 2).max(2);
            let crop_h = (((1.0 - top - bottom) * src_h).round() as i64 / 2 * 2).max(2);
            let crop_x = ((left * src_w).round() as i64).max(0);
            let crop_y = ((top * src_h).round() as i64).max(0);
            let cropping = (left + right + top + bottom) > 0.001;
            if cropping {
                filters.push(format!("crop={crop_w}:{crop_h}:{crop_x}:{crop_y}"));
            }

            // Colour correction sits after the crop and before the (GPU) rescale, so
            // the CPU-bound `eq`/`hue` filters touch as few pixels as possible.
            filters.extend(adjustments.ffmpeg_filters());

            let out_w = scaled.width as i64;
            let out_h = scaled.height as i64;
            let cur_w = if cropping {
                crop_w
            } else {
                src_w.round() as i64
            };
            let cur_h = if cropping {
                crop_h
            } else {
                src_h.round() as i64
            };
            if out_w != cur_w || out_h != cur_h {
                // libplacebo ewa_lanczossharp: GPU up/downscale (the chosen "E" recipe).
                filters.push(format!(
                    "libplacebo=w={out_w}:h={out_h}:upscaler=ewa_lanczossharp"
                ));
            }

            if !speed.is_normal() {
                filters.push(format!("setpts=PTS/{:.4}", speed.factor()));
            }

            filters.push(format!(
                "fps={}/{}",
                framerate.nominator,
                framerate.denominator.max(1)
            ));

            let video_encoding = output_format.video_encoding.unwrap();
            let (sw, hw) = video_encoding.ffmpeg_encoders();
            let encoder = if prefer_gpu {
                hw.filter(|h| ffmpeg_has_encoder(h)).unwrap_or(sw)
            } else {
                sw
            };
            log::info!(
                "ffmpeg export encoder: {} [{}]",
                encoder,
                if encoder.contains("nvenc") {
                    "GPU/NVENC"
                } else {
                    "CPU/software"
                }
            );

            let quality_args = ffmpeg_quality_args(encoder, output_format.quality);
            let audio_codec = if mute {
                None
            } else {
                output_format.audio_encoding.map(|a| a.ffmpeg_codec())
            };

            let ss = inpoint_ns as f64 / 1_000_000_000.0;
            let t = duration_ns as f64 / 1_000_000_000.0;
            let source_selection_ms = (duration_ns / 1_000_000).max(1);
            let selection_ms = speed.output_duration_ms(source_selection_ms);
            let fps = framerate.nominator as f64 / framerate.denominator.max(1) as f64;

            // Stage weights, in milliseconds of material processed. The reversed pass
            // re-encodes the whole selection again; the final concat is a stream copy,
            // roughly an order of magnitude faster.
            let reverse_weight = if repeat.is_boomerang() {
                selection_ms
            } else {
                0
            };
            let concat_weight = if repeat.is_off() {
                0
            } else {
                selection_ms / 10 + 1
            };
            let total_ms = selection_ms + reverse_weight + concat_weight;

            let mut temps = TempFiles::default();
            let ext = output_path
                .extension()
                .and_then(|e| e.to_str())
                .unwrap_or_else(|| output_format.container_format.extension())
                .to_owned();

            // With nothing to concatenate afterwards, the first pass *is* the export.
            let forward_path = if repeat.is_off() {
                output_path.clone()
            } else {
                temps.reserve(&output_path, "fwd", &ext)
            };

            // Pass 1: source -> forward render at the target codec, size and framerate.
            let mut cmd = ffmpeg_base();
            cmd.args(["-ss", &format!("{ss}"), "-t", &format!("{t}")]);
            cmd.arg("-i").arg(&input_path);
            cmd.args(["-vf", &filters.join(",")]);
            cmd.arg("-c:v").arg(encoder);
            cmd.args(&quality_args);
            match audio_codec {
                Some(codec) => {
                    if let Some(filter) = speed.ffmpeg_audio_filter() {
                        cmd.args(["-af", filter]);
                    }
                    cmd.arg("-c:a").arg(codec);
                }
                None => {
                    cmd.arg("-an");
                }
            }
            cmd.arg(&forward_path);

            if run_ffmpeg_stage(
                &mut cmd,
                selection_ms,
                0,
                selection_ms,
                total_ms,
                &sender,
                &running_flag,
            ) != StageOutcome::Done
            {
                let _ = sender.send_blocking(Err(()));
                return;
            }

            if repeat.is_off() {
                let _ = sender.send_blocking(Ok((total_ms, total_ms)));
                return;
            }

            let mut done_ms = selection_ms;

            // Pass 2 (boomerang only): reverse the forward render. ffmpeg's `reverse`
            // buffers every frame of its input, so it runs over chunks sized to a memory
            // budget, and the chunks are later listed back to front.
            let mut reversed: Vec<PathBuf> = vec![];
            if repeat.is_boomerang() {
                let chunk_ms = reverse_chunk_ms(scaled, fps);
                let chunks = selection_ms.div_ceil(chunk_ms).max(1);
                log::info!(
                    "boomerang: reversing {selection_ms} ms as {chunks} chunk(s) of up to {chunk_ms} ms"
                );

                for i in 0..chunks {
                    let start_ms = i * chunk_ms;
                    let len_ms = chunk_ms.min(selection_ms - start_ms);
                    let path = temps.reserve(&output_path, &format!("rev{i}"), &ext);

                    let mut cmd = ffmpeg_base();
                    cmd.args([
                        "-ss",
                        &format!("{:.3}", start_ms as f64 / 1000.0),
                        "-t",
                        &format!("{:.3}", len_ms as f64 / 1000.0),
                    ]);
                    cmd.arg("-i").arg(&forward_path);
                    cmd.args(["-vf", "reverse"]);
                    cmd.arg("-c:v").arg(encoder);
                    cmd.args(&quality_args);
                    match audio_codec {
                        Some(codec) => {
                            cmd.args(["-af", "areverse"]);
                            cmd.arg("-c:a").arg(codec);
                        }
                        None => {
                            cmd.arg("-an");
                        }
                    }
                    cmd.arg(&path);

                    // Each chunk gets its share of the reversed pass's progress slice.
                    let span = reverse_weight * len_ms / selection_ms;
                    if run_ffmpeg_stage(
                        &mut cmd,
                        len_ms,
                        done_ms,
                        span,
                        total_ms,
                        &sender,
                        &running_flag,
                    ) != StageOutcome::Done
                    {
                        let _ = sender.send_blocking(Err(()));
                        return;
                    }

                    done_ms += span;
                    reversed.push(path);
                }

                // The last chunk of the forward render plays first when reversed.
                reversed.reverse();
            }

            // Final pass: join one entry per cycle. Every part came out of the same
            // encoder with the same settings, so this is a stream copy.
            let list_path = temps.reserve(&output_path, "concat", "txt");
            let mut list = String::new();
            for _ in 0..repeat.cycles() {
                list.push_str(&concat_entry(&forward_path));
                for chunk in &reversed {
                    list.push_str(&concat_entry(chunk));
                }
            }
            if let Err(err) = std::fs::File::create(&list_path)
                .and_then(|mut file| file.write_all(list.as_bytes()))
            {
                log::error!("could not write the concat list: {err}");
                let _ = sender.send_blocking(Err(()));
                return;
            }

            let output_ms = repeat.output_duration_ms(selection_ms);
            let concat_span = total_ms.saturating_sub(done_ms);

            let mut cmd = ffmpeg_base();
            cmd.args(["-f", "concat", "-safe", "0"]);
            cmd.arg("-i").arg(&list_path);
            cmd.args(["-c", "copy"]);
            cmd.arg(&output_path);

            let mut outcome = run_ffmpeg_stage(
                &mut cmd,
                output_ms,
                done_ms,
                concat_span,
                total_ms,
                &sender,
                &running_flag,
            );

            // A stream copy needs compatible stream parameters across the parts. If the
            // muxer still refuses them, re-encode the joined result instead of losing
            // the whole export.
            if outcome == StageOutcome::Failed {
                log::warn!("concat stream copy failed; re-encoding the joined result");

                let mut cmd = ffmpeg_base();
                cmd.args(["-f", "concat", "-safe", "0"]);
                cmd.arg("-i").arg(&list_path);
                cmd.arg("-c:v").arg(encoder);
                cmd.args(&quality_args);
                match audio_codec {
                    Some(codec) => {
                        cmd.arg("-c:a").arg(codec);
                    }
                    None => {
                        cmd.arg("-an");
                    }
                }
                cmd.arg(&output_path);

                outcome = run_ffmpeg_stage(
                    &mut cmd,
                    output_ms,
                    done_ms,
                    concat_span,
                    total_ms,
                    &sender,
                    &running_flag,
                );
            }

            let _ = sender.send_blocking(if outcome == StageOutcome::Done {
                Ok((total_ms, total_ms))
            } else {
                Err(())
            });
        });
    }
}

fn ffmpeg_filter_chain(
    oriented: Dimensions<f64>,
    crop: (f64, f64, f64, f64),
    scaled: Dimensions<u32>,
    orientation: VideoOrientation,
    adjustments: ColorAdjustments,
    speed: PlaybackSpeed,
    framerate: Framerate,
) -> Vec<String> {
    let (top, right, bottom, left) = crop;
    let mut filters = orientation
        .ffmpeg_filters()
        .iter()
        .map(|filter| filter.to_string())
        .collect::<Vec<_>>();

    let crop_w = (((1.0 - left - right) * oriented.width).round() as i64 / 2 * 2).max(2);
    let crop_h = (((1.0 - top - bottom) * oriented.height).round() as i64 / 2 * 2).max(2);
    let cropping = (left + right + top + bottom) > 0.001;
    if cropping {
        let crop_x = ((left * oriented.width).round() as i64).max(0);
        let crop_y = ((top * oriented.height).round() as i64).max(0);
        filters.push(format!("crop={crop_w}:{crop_h}:{crop_x}:{crop_y}"));
    }

    filters.extend(adjustments.ffmpeg_filters());

    let current_width = if cropping {
        crop_w
    } else {
        oriented.width.round() as i64
    };
    let current_height = if cropping {
        crop_h
    } else {
        oriented.height.round() as i64
    };
    if scaled.width as i64 != current_width || scaled.height as i64 != current_height {
        filters.push(format!(
            "libplacebo=w={}:h={}:upscaler=ewa_lanczossharp",
            scaled.width, scaled.height
        ));
    }
    if !speed.is_normal() {
        filters.push(format!("setpts=PTS/{:.4}", speed.factor()));
    }
    filters.push(format!(
        "fps={}/{}",
        framerate.nominator,
        framerate.denominator.max(1)
    ));
    filters
}

fn unique_segment_path(folder: &Path, input: &Path, index: usize, extension: &str) -> PathBuf {
    let stem = input
        .file_stem()
        .and_then(|stem| stem.to_str())
        .filter(|stem| !stem.is_empty())
        .unwrap_or("clip");
    let base = format!("{stem}-section-{:02}", index + 1);
    let mut candidate = folder.join(format!("{base}.{extension}"));
    let mut copy = 2;
    while candidate.exists() {
        candidate = folder.join(format!("{base}-{copy}.{extension}"));
        copy += 1;
    }
    candidate
}

/// Sets a `gdouble` child property on a GES effect, warning instead of failing when
/// the underlying element does not expose it.
fn set_child_property_f64(effect: &Effect, property: &str, value: f64) {
    if ges::prelude::TrackElementExt::set_child_property(effect, property, &value.to_value())
        .is_err()
    {
        log::warn!("could not set effect property {property}");
    }
}

/// Intermediate render files, removed when the export ends for any reason — success,
/// failure or cancellation.
#[derive(Default)]
struct TempFiles(Vec<PathBuf>);

impl TempFiles {
    /// Reserves a hidden sibling of `near`, keeping intermediate files on the same
    /// filesystem as the output.
    fn reserve(&mut self, near: &Path, tag: &str, ext: &str) -> PathBuf {
        let dir = near.parent().unwrap_or_else(|| Path::new("."));
        let path = dir.join(format!(".clips-{}-{tag}.{ext}", std::process::id()));
        self.0.push(path.clone());
        path
    }
}

impl Drop for TempFiles {
    fn drop(&mut self) {
        for path in &self.0 {
            let _ = std::fs::remove_file(path);
        }
    }
}

/// How one ffmpeg invocation ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StageOutcome {
    Done,
    /// The user cancelled; the process was killed.
    Cancelled,
    Failed,
}

/// The shared ffmpeg invocation prefix. Errors go to the app's own stderr, which keeps
/// failures diagnosable without a pipe this code would have to keep draining.
fn ffmpeg_base() -> Command {
    let mut cmd = Command::new("ffmpeg");
    cmd.args(["-y", "-nostdin", "-hide_banner", "-loglevel", "error"]);
    cmd
}

/// Runs one ffmpeg invocation, mapping its `-progress` output onto the
/// `[base_ms, base_ms + span_ms]` slice of an export that totals `total_ms`.
#[allow(clippy::too_many_arguments)]
fn run_ffmpeg_stage(
    cmd: &mut Command,
    stage_ms: u64,
    base_ms: u64,
    span_ms: u64,
    total_ms: u64,
    sender: &async_channel::Sender<Result<(u64, u64), ()>>,
    running_flag: &Arc<AtomicBool>,
) -> StageOutcome {
    cmd.args(["-progress", "pipe:1", "-nostats"]);
    cmd.stdout(Stdio::piped()).stderr(Stdio::inherit());

    log::debug!("ffmpeg stage: {cmd:?}");

    let mut child = match cmd.spawn() {
        Ok(child) => child,
        Err(err) => {
            log::error!("could not start ffmpeg: {err}");
            return StageOutcome::Failed;
        }
    };

    let stdout = child.stdout.take().expect("ffmpeg stdout");
    let mut ended = false;

    for line in BufReader::new(stdout).lines().map_while(Result::ok) {
        if !running_flag.load(Ordering::SeqCst) {
            let _ = child.kill();
            let _ = child.wait();
            return StageOutcome::Cancelled;
        }
        if let Some(value) = line.strip_prefix("out_time_us=") {
            if let Ok(us) = value.trim().parse::<u64>() {
                let fraction = ((us / 1000) as f64 / stage_ms.max(1) as f64).clamp(0.0, 1.0);
                let done = base_ms + (span_ms as f64 * fraction) as u64;
                // Never report done == total: that is the export's completion signal.
                let _ = sender.send_blocking(Ok((done.min(total_ms.saturating_sub(1)), total_ms)));
            }
        } else if line.starts_with("progress=end") {
            ended = true;
        }
    }

    if ended && child.wait().map(|status| status.success()).unwrap_or(false) {
        StageOutcome::Done
    } else {
        StageOutcome::Failed
    }
}

/// One line of an ffmpeg concat demuxer list.
fn concat_entry(path: &Path) -> String {
    format!("file '{}'\n", path.to_string_lossy().replace('\'', "'\\''"))
}

/// Chunk length for the reversed (boomerang) pass. ffmpeg's `reverse` filter buffers
/// every frame of its input, so the chunk is sized against a fixed memory budget
/// instead of growing with the clip length.
fn reverse_chunk_ms(scaled: Dimensions<u32>, fps: f64) -> u64 {
    const BUDGET_BYTES: u64 = 1_200_000_000;
    // yuv420p is 1.5 bytes per pixel; 2 leaves room for alignment and frame overhead.
    let frame_bytes = (scaled.width as u64 * scaled.height as u64 * 2).max(1);
    let frames = (BUDGET_BYTES / frame_bytes).max(1);
    let ms = (frames as f64 / fps.max(1.0) * 1000.0) as u64;
    ms.clamp(1_000, 60_000)
}

/// Whether an ffmpeg hardware encoder is actually usable on this machine.
/// Merely appearing in `ffmpeg -encoders` is not enough: for example, FFmpeg may
/// expose `av1_nvenc` on an RTX 30-series GPU even though that generation cannot
/// initialize the encoder. Each known NVENC codec is probed once with a 64×64 frame.
pub(crate) fn ffmpeg_has_encoder(name: &str) -> bool {
    static H264_NVENC: OnceLock<bool> = OnceLock::new();
    static HEVC_NVENC: OnceLock<bool> = OnceLock::new();
    static AV1_NVENC: OnceLock<bool> = OnceLock::new();

    let cache = match name {
        "h264_nvenc" => &H264_NVENC,
        "hevc_nvenc" => &HEVC_NVENC,
        "av1_nvenc" => &AV1_NVENC,
        _ => return false,
    };
    *cache.get_or_init(|| probe_ffmpeg_encoder(name))
}

fn probe_ffmpeg_encoder(name: &str) -> bool {
    let available = Command::new("ffmpeg")
        .args(["-hide_banner", "-encoders"])
        .output()
        .map(|output| String::from_utf8_lossy(&output.stdout).contains(name))
        .unwrap_or(false);
    if !available {
        return false;
    }

    Command::new("ffmpeg")
        .args([
            "-hide_banner",
            "-loglevel",
            "error",
            "-f",
            "lavfi",
            "-i",
            "color=c=black:s=64x64:r=1",
            "-frames:v",
            "1",
            "-an",
            "-c:v",
            name,
            "-f",
            "null",
            "-",
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}

/// Rate-control args for constant-quality encoding (auto-scales file size with
/// resolution, fixing the fixed-bitrate bloat).
fn ffmpeg_quality_args(encoder: &str, quality: Quality) -> Vec<String> {
    // CQ/CRF: lower = better quality + bigger file. These target visually-high
    // quality without exceeding typical source bitrates (CQ20 was archival-overkill
    // and produced files larger than the original).
    let cq = match quality {
        Quality::High => 23,
        Quality::Medium => 27,
        Quality::Low => 31,
        Quality::Unchanged => 24,
    };
    let s = |n: i32| n.to_string();
    match encoder {
        "h264_nvenc" | "hevc_nvenc" | "av1_nvenc" => vec![
            "-rc".into(),
            "vbr".into(),
            "-cq".into(),
            s(cq),
            "-b:v".into(),
            "0".into(),
        ],
        "libx264" | "libx265" => vec!["-crf".into(), s(cq)],
        "libsvtav1" => vec!["-crf".into(), s(cq + 10)],
        "libvpx-vp9" => vec!["-crf".into(), s(cq), "-b:v".into(), "0".into()],
        "libvpx" => vec!["-crf".into(), s(cq), "-b:v".into(), "2M".into()],
        _ => vec![],
    }
}
