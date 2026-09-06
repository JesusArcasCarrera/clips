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
    segments::{ClipRange, ClipSegment, SegmentExportMode},
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
        pub sequence_clips: RefCell<Vec<ges::UriClip>>,
        pub active_segment: Cell<usize>,
        pub sequence_segments: RefCell<Vec<ClipSegment>>,
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

/// An already-probed copy export. `audio` means every input has the same
/// compatible first audio stream and it should be mapped to the output.
#[derive(Debug, Clone, Copy)]
pub(crate) struct StreamCopyPlan {
    audio: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct AudioProbe {
    codec: String,
    sample_rate: String,
    channels: String,
    layout: String,
}

#[derive(Debug, Clone)]
struct MediaProbe {
    format: String,
    video_codec: String,
    width: u32,
    height: u32,
    fps_n: u32,
    fps_d: u32,
    audio: Option<AudioProbe>,
    duration_ms: u64,
    keyframes_ms: Vec<u64>,
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
        self.imp().sequence_clips.replace(vec![]);
        self.imp().sequence_segments.replace(vec![]);
        self.imp().active_segment.set(0);
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
        self.imp().sequence_clips.replace(vec![clip.clone()]);
        self.imp().clip.replace(Some(clip));
        self.imp().sequence_segments.replace(vec![]);
        self.imp().active_segment.set(0);

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

    /// Load all sources into one GES timeline.  The sequence remains a single
    /// layer with no overlaps; each clip keeps its source inpoint and duration.
    pub async fn load_sequence(&self, segments: &[ClipSegment]) -> Result<(), ()> {
        if segments.is_empty() {
            return Err(());
        }

        let mut clips = Vec::with_capacity(segments.len());
        let mut offset_ms = 0;
        for segment in segments {
            let uri = url::Url::from_file_path(segment.source_path())
                .map_err(|_| ())?
                .to_string();
            let clip = self
                .load_ges_clip(&uri)
                .await
                .map_err(|_| ())?
                .extract()
                .map_err(|_| ())?
                .dynamic_cast::<ges::UriClip>()
                .map_err(|_| ())?;
            clip.set_inpoint(ClockTime::from_mseconds(segment.range.start_ms));
            clip.set_duration(Some(ClockTime::from_mseconds(segment.duration_ms())));
            clip.set_start(ClockTime::from_mseconds(offset_ms));
            offset_ms = offset_ms.saturating_add(segment.duration_ms());
            clips.push(clip);
        }

        let first = clips.first().cloned().ok_or(())?;
        let first_segment = segments.first().ok_or(())?;
        let (dimensions, _framerate, has_audio) =
            get_info(first_segment.source.to_string_lossy().into_owned()).ok_or(())?;

        self.imp().sequence_clips.replace(clips.clone());
        self.imp().sequence_segments.replace(segments.to_vec());
        self.imp().active_segment.set(0);
        self.imp().clip.replace(Some(first));
        self.imp().inpoint.set(first_segment.range.start_ms);
        self.imp().outpoint.set(first_segment.range.end_ms);
        self.imp().current_dimensions.set(Some(dimensions));
        self.imp().path.replace(first_segment.source.clone());
        self.imp().mute.set(!has_audio);
        self.refresh_ui();
        Ok(())
    }

    /// Select the source used by the trim handles while keeping the complete
    /// sequence alive in the playback pipeline.
    pub fn set_active_segment(&self, index: usize) {
        let Some(segment) = self.imp().sequence_segments.borrow().get(index).cloned() else {
            return;
        };
        let Some(clip) = self.imp().sequence_clips.borrow().get(index).cloned() else {
            return;
        };
        self.imp().active_segment.set(index);
        self.imp().clip.replace(Some(clip));
        self.imp().path.replace(segment.source);
        self.imp().inpoint.set(segment.range.start_ms);
        self.imp().outpoint.set(segment.range.end_ms);
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
            let sequence_offset = self
                .imp()
                .sequence_clips
                .borrow()
                .get(self.imp().active_segment.get())
                .map(|clip| clip.start().mseconds())
                .unwrap_or(0);
            if let Err(err) = pipeline.seek(
                self.playback_rate(),
                SeekFlags::FLUSH | SeekFlags::ACCURATE,
                gst::SeekType::Set,
                ClockTime::from_mseconds(sequence_offset.saturating_add(position_ms)),
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

        let layer = timeline.append_layer();
        let sequence_clips = self.imp().sequence_clips.borrow();
        if sequence_clips.is_empty() {
            let clip = self.imp().clip.borrow();
            layer.add_clip(clip.as_ref().unwrap()).unwrap();
        } else {
            for clip in sequence_clips.iter() {
                layer.add_clip(clip).unwrap();
            }
        }

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
                        let offset = this
                            .imp()
                            .sequence_clips
                            .borrow()
                            .get(this.imp().active_segment.get())
                            .map(|clip| clip.start().mseconds())
                            .unwrap_or(0);
                        let active_end = offset.saturating_add(
                            this.imp()
                                .outpoint
                                .get()
                                .saturating_sub(this.imp().inpoint.get()),
                        );
                        if p >= offset && p <= active_end {
                            this.emit_by_name::<()>(
                                "set-position",
                                &[&(p.saturating_sub(offset) + this.imp().inpoint.get())],
                            );
                        }
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
        if let Some(segment) = self
            .imp()
            .sequence_segments
            .borrow_mut()
            .get_mut(self.imp().active_segment.get())
        {
            segment.range = ClipRange::new(start, end);
        }
        let mut offset_ms = 0;
        for clip in self.imp().sequence_clips.borrow().iter() {
            clip.set_start(ClockTime::from_mseconds(offset_ms));
            offset_ms = offset_ms.saturating_add(clip.duration().mseconds());
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

    /// Applies image correction to the live preview. Colour and sharpness are
    /// managed independently: a sharpness negotiation failure must never disable
    /// brightness, contrast, saturation, hue or gamma.
    pub fn set_color_adjustments(&self, adjustments: ColorAdjustments) {
        self.imp().adjustments.set(adjustments);

        if self.imp().clip.borrow().is_none() || self.imp().pipeline.borrow().is_none() {
            return;
        }
        let needs_colour = !adjustments.colour_is_neutral();
        let has_colour = self.imp().balance.borrow().is_some();
        let needs_sharpness = adjustments.has_sharpness();
        let has_sharpness = self.imp().sharpness.borrow().is_some();
        let topology_changed = needs_colour != has_colour || needs_sharpness != has_sharpness;

        if !topology_changed && adjustments.is_neutral() {
            return;
        }

        // CPU effects cannot be inserted reliably into an already-negotiated
        // hardware-decoding pipeline (for example one producing CUDAMemory).
        // Rebuild only when an effect appears or disappears; ordinary slider
        // movement below remains a cheap property update.
        let source_position = topology_changed.then(|| {
            self.imp()
                .pipeline
                .borrow()
                .as_ref()
                .and_then(|pipeline| pipeline.query_position::<ClockTime>())
                .map(ClockTime::mseconds)
                .unwrap_or(0)
                .saturating_add(self.imp().inpoint.get())
                .min(self.imp().outpoint.get())
        });
        if topology_changed {
            self.kill();
        }

        if needs_colour {
            self.ensure_colour_effects();

            if let Some(balance) = self.imp().balance.borrow().as_ref() {
                set_child_property_f64(balance, "brightness", adjustments.brightness);
                set_child_property_f64(balance, "contrast", adjustments.contrast);
                set_child_property_f64(balance, "saturation", adjustments.saturation);
                set_child_property_f64(balance, "hue", adjustments.gst_hue());
            }
            if let Some(gamma) = self.imp().gamma.borrow().as_ref() {
                set_child_property_f64(gamma, "gamma", adjustments.gamma);
            }
        } else {
            self.remove_colour_effects();
        }

        self.update_sharpness_effect(adjustments);
        if let Some(position) = source_position {
            self.refresh_ui();
            self.quiet_seek(position);
        } else {
            self.commit();
        }
    }

    /// The colour correction currently shown in the preview.
    pub fn color_adjustments(&self) -> ColorAdjustments {
        self.imp().adjustments.get()
    }

    fn ensure_colour_effects(&self) {
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
        if let Some(clip) = self.imp().clip.borrow().as_ref() {
            if let Err(err) = clip.add_top_effect(&balance, 0) {
                log::warn!("could not add videobalance to the preview: {err}");
                return;
            }
            if let Err(err) = clip.add_top_effect(&gamma, 0) {
                log::warn!("could not add gamma to the preview: {err}");
                clip.remove(&balance).ok();
                return;
            }
        }

        self.imp().balance.replace(Some(balance));
        self.imp().gamma.replace(Some(gamma));
    }

    fn remove_colour_effects(&self) {
        let balance = self.imp().balance.borrow_mut().take();
        let gamma = self.imp().gamma.borrow_mut().take();
        let clip = self.imp().clip.borrow();
        let Some(clip) = clip.as_ref() else {
            return;
        };

        for (name, effect) in [("videobalance", balance), ("gamma", gamma)] {
            if let Some(effect) = effect {
                if let Err(err) = clip.remove(&effect) {
                    log::warn!("could not remove {name} from the preview: {err}");
                }
            }
        }
    }

    fn update_sharpness_effect(&self, adjustments: ColorAdjustments) {
        if !adjustments.has_sharpness() {
            let previous = self.imp().sharpness.borrow_mut().take();
            if let (Some(clip), Some(effect)) =
                (self.imp().clip.borrow().as_ref(), previous.as_ref())
            {
                if let Err(err) = clip.remove(effect) {
                    log::warn!("could not remove sharpness from the preview: {err}");
                }
            }
            return;
        }

        if let Some(sharpness) = self.imp().sharpness.borrow().as_ref() {
            set_child_property_f64(sharpness, "sigma", adjustments.gst_sharpen_sigma());
            return;
        }

        let Ok(sharpness) = ges::Effect::new(ColorAdjustments::GST_SHARPEN_EFFECT) else {
            log::warn!("gaussianblur is unavailable; sharpness will not preview");
            return;
        };
        set_child_property_f64(&sharpness, "sigma", adjustments.gst_sharpen_sigma());

        if let Some(clip) = self.imp().clip.borrow().as_ref() {
            if let Err(err) = clip.add_top_effect(&sharpness, 0) {
                log::warn!("could not add sharpness to the preview: {err}");
                return;
            }
        }
        self.imp().sharpness.replace(Some(sharpness));
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
        segments: Vec<ClipSegment>,
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

        // Repackage compatible streams without decoding.  The probe is deliberately
        // strict: mixed stream layouts, non-keyframe cuts and any visual/audio
        // transform go through the established render path below.
        if let Ok(plan) = Self::stream_copy_plan(
            &output_format,
            framerate,
            scaled_dimension,
            orientation,
            (top, right, bottom, left),
            mute,
            repeat,
            speed,
            &segments,
        ) {
            Self::save_ffmpeg_copy(
                output_path,
                output_format,
                plan,
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

        // Fast path: render with ffmpeg (NVENC + libplacebo). GES stays as fallback
        // for the cases ffmpeg doesn't cover here (GIF, "keep as-is").
        if Self::ffmpeg_can_handle(&output_format) {
            if segments.len() > 1 {
                Self::save_ffmpeg_segments(
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

            let segment = segments.first().cloned().unwrap_or_else(|| {
                let path = input_path.clone();
                ClipSegment::from_range(
                    path,
                    ClipRange::new(inpoint.mseconds(), inpoint.mseconds() + duration.mseconds()),
                )
            });
            let range = segment.range;
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

    /// Probe the inputs before choosing the lossless path.  Keeping this decision
    /// in one function lets the export worker and the UI indicator report the same
    /// answer and, importantly, keeps mixed audio sources on the normaliser path.
    pub(crate) fn stream_copy_reason(
        output_format: &OutputFormat,
        framerate: Framerate,
        scaled: Dimensions<u32>,
        orientation: VideoOrientation,
        crop: (f64, f64, f64, f64),
        mute: bool,
        repeat: Repeat,
        speed: PlaybackSpeed,
        segments: &[ClipSegment],
    ) -> Result<(), &'static str> {
        Self::stream_copy_plan(
            output_format,
            framerate,
            scaled,
            orientation,
            crop,
            mute,
            repeat,
            speed,
            segments,
        )
        .map(|_| ())
    }

    fn stream_copy_plan(
        output_format: &OutputFormat,
        framerate: Framerate,
        scaled: Dimensions<u32>,
        orientation: VideoOrientation,
        crop: (f64, f64, f64, f64),
        mute: bool,
        repeat: Repeat,
        speed: PlaybackSpeed,
        segments: &[ClipSegment],
    ) -> Result<StreamCopyPlan, &'static str> {
        if segments.is_empty() {
            return Err("no sections selected");
        }
        if !Self::ffmpeg_can_handle(output_format) {
            return Err("the selected container uses the GES renderer");
        }
        if output_format.video_encoding.is_none() {
            return Err("the output video codec is not selectable");
        }
        if !Self::orientation_is_identity(orientation) {
            return Err("orientation changes require recoding");
        }
        if crop.0.abs() > 0.001
            || crop.1.abs() > 0.001
            || crop.2.abs() > 0.001
            || crop.3.abs() > 0.001
        {
            return Err("cropping requires recoding");
        }
        if !speed.is_normal() {
            return Err("speed changes require recoding");
        }
        if !repeat.is_off() {
            return Err("repetition requires recoding");
        }

        let probes = segments
            .iter()
            .map(|segment| Self::probe_media(&segment.source))
            .collect::<Result<Vec<_>, _>>()?;
        let video_codec = output_format.video_encoding.unwrap();
        let expected_video = Self::video_codec_name(video_codec);
        let expected_container = Self::output_container_name(output_format.container_format);
        let first = probes.first().unwrap();

        for (segment, media) in segments.iter().zip(&probes) {
            if media.video_codec != expected_video {
                return Err("video codecs are incompatible with stream copy");
            }
            if media.width != scaled.width || media.height != scaled.height {
                return Err("resolution changes require recoding");
            }
            if media.fps_n as u64 * framerate.denominator as u64
                != framerate.nominator as u64 * media.fps_d as u64
            {
                return Err("FPS changes require recoding");
            }
            if media
                .format
                .split(',')
                .all(|name| !expected_container(name))
            {
                return Err("the source container is not compatible with the output");
            }
            if !Self::keyframe_aligned(media, segment.range.start_ms, segment.range.end_ms) {
                return Err("cuts must start and end on video keyframes");
            }
        }

        let audio = if mute {
            false
        } else {
            let all_have_audio = probes.iter().all(|media| media.audio.is_some());
            let none_have_audio = probes.iter().all(|media| media.audio.is_none());
            if none_have_audio {
                false
            } else if !all_have_audio {
                return Err("mixed audio tracks require normalization");
            } else {
                let first_audio = first.audio.as_ref().unwrap();
                let Some(audio_encoding) = output_format.audio_encoding else {
                    return Err("the output audio codec is not selectable");
                };
                if first_audio.codec != Self::audio_codec_name(audio_encoding)
                    || probes
                        .iter()
                        .any(|media| media.audio.as_ref() != Some(first_audio))
                {
                    return Err("audio tracks are incompatible with stream copy");
                }
                true
            }
        };

        Ok(StreamCopyPlan { audio })
    }

    fn orientation_is_identity(orientation: VideoOrientation) -> bool {
        orientation == VideoOrientation::Identity
    }

    fn video_codec_name(codec: VideoEncoding) -> &'static str {
        match codec {
            VideoEncoding::Av1 => "av1",
            VideoEncoding::Vp8 => "vp8",
            VideoEncoding::Vp9 => "vp9",
            VideoEncoding::H264 => "h264",
            VideoEncoding::H265 => "hevc",
            VideoEncoding::Gif => "gif",
        }
    }

    fn audio_codec_name(codec: crate::profiles::AudioEncoding) -> &'static str {
        match codec {
            crate::profiles::AudioEncoding::Aac => "aac",
            crate::profiles::AudioEncoding::Ac3 => "ac3",
            crate::profiles::AudioEncoding::Opus => "opus",
            crate::profiles::AudioEncoding::Vorbis => "vorbis",
            crate::profiles::AudioEncoding::Flac => "flac",
        }
    }

    fn output_container_name(container: ContainerFormat) -> fn(&str) -> bool {
        match container {
            ContainerFormat::Mpeg => {
                |name| matches!(name, "mov" | "mp4" | "m4a" | "3gp" | "3g2" | "mj2")
            }
            ContainerFormat::Matroska => |name| name == "matroska",
            ContainerFormat::WebM | ContainerFormat::Best => |name| name == "webm",
            ContainerFormat::Same | ContainerFormat::GifContainer => |_| false,
        }
    }

    fn probe_media(path: &Path) -> Result<MediaProbe, &'static str> {
        let video = Self::ffprobe_csv(
            path,
            &["-select_streams", "v:0"],
            "stream=codec_name,width,height,r_frame_rate",
        )?;
        let video_fields = video.trim().split('|').collect::<Vec<_>>();
        if video_fields.len() < 4 {
            return Err("ffprobe could not read the video stream");
        }
        let (fps_n, fps_d) = Self::parse_fraction(video_fields[3]).ok_or("invalid source FPS")?;
        let audio = Self::ffprobe_csv(
            path,
            &["-select_streams", "a:0"],
            "stream=codec_name,sample_rate,channels,channel_layout",
        )
        .ok()
        .and_then(|value| {
            let fields = value.trim().split('|').collect::<Vec<_>>();
            (fields.len() >= 4 && !fields[0].is_empty()).then(|| AudioProbe {
                codec: fields[0].to_owned(),
                sample_rate: fields[1].to_owned(),
                channels: fields[2].to_owned(),
                layout: fields[3].to_owned(),
            })
        });
        let format = Self::ffprobe_csv(path, &[], "format=format_name")?;
        let duration_ms = Self::ffprobe_csv(path, &[], "format=duration")
            .ok()
            .and_then(|value| value.trim().parse::<f64>().ok())
            .filter(|value| value.is_finite() && *value > 0.0)
            .map(|value| (value * 1_000.0).round() as u64)
            .ok_or("invalid source duration")?;
        let keyframes_ms = Self::ffprobe_keyframes(path)?;
        Ok(MediaProbe {
            format,
            video_codec: video_fields[0].to_owned(),
            width: video_fields[1]
                .parse()
                .map_err(|_| "invalid source width")?,
            height: video_fields[2]
                .parse()
                .map_err(|_| "invalid source height")?,
            fps_n,
            fps_d,
            audio,
            duration_ms,
            keyframes_ms,
        })
    }

    fn ffprobe_csv(path: &Path, extra: &[&str], entries: &str) -> Result<String, &'static str> {
        let mut command = Command::new("ffprobe");
        command.args(["-v", "error"]);
        command.args(extra);
        command.args(["-show_entries", entries, "-of", "csv=s=|:p=0"]);
        let output = command
            .arg(path)
            .output()
            .map_err(|_| "ffprobe is unavailable")?;
        if !output.status.success() {
            return Err("ffprobe could not inspect the source");
        }
        let value = String::from_utf8_lossy(&output.stdout).trim().to_owned();
        if value.is_empty() {
            Err("ffprobe returned no stream data")
        } else {
            Ok(value)
        }
    }

    fn ffprobe_keyframes(path: &Path) -> Result<Vec<u64>, &'static str> {
        let output = Command::new("ffprobe")
            .args([
                "-v",
                "error",
                "-select_streams",
                "v:0",
                "-skip_frame",
                "nokey",
                "-show_entries",
                "frame=best_effort_timestamp_time",
                "-of",
                "csv=p=0",
            ])
            .arg(path)
            .output()
            .map_err(|_| "ffprobe is unavailable")?;
        if !output.status.success() {
            return Err("ffprobe could not inspect keyframes");
        }
        let values = String::from_utf8_lossy(&output.stdout)
            .lines()
            .filter_map(|line| line.trim().parse::<f64>().ok())
            .filter(|value| value.is_finite() && *value >= 0.0)
            .map(|value| (value * 1_000.0).round() as u64)
            .collect::<Vec<_>>();
        if values.is_empty() {
            Err("ffprobe found no video keyframes")
        } else {
            Ok(values)
        }
    }

    fn parse_fraction(value: &str) -> Option<(u32, u32)> {
        let (numerator, denominator) = value.trim().split_once('/')?;
        Some((numerator.parse().ok()?, denominator.parse().ok()?))
    }

    fn keyframe_aligned(media: &MediaProbe, start_ms: u64, end_ms: u64) -> bool {
        const TOLERANCE_MS: u64 = 4;
        let nearest = |target: u64| {
            media
                .keyframes_ms
                .iter()
                .map(|keyframe| keyframe.abs_diff(target))
                .min()
                .unwrap_or(u64::MAX)
        };
        let start_ok = start_ms == 0 || nearest(start_ms) <= TOLERANCE_MS;
        let end_ok =
            end_ms >= media.duration_ms.saturating_sub(50) || nearest(end_ms) <= TOLERANCE_MS;
        start_ok && end_ok
    }

    /// Repackage one or more keyframe-aligned ranges.  If a muxer or unusual
    /// source defeats the preflight, hand the exact same request to the existing
    /// renderer instead of exposing a failed export to the user.
    #[allow(clippy::too_many_arguments)]
    fn save_ffmpeg_copy(
        output_target: PathBuf,
        output_format: OutputFormat,
        plan: StreamCopyPlan,
        framerate: Framerate,
        oriented: Dimensions<f64>,
        crop: (f64, f64, f64, f64),
        scaled: Dimensions<u32>,
        orientation: VideoOrientation,
        mute: bool,
        prefer_gpu: bool,
        adjustments: ColorAdjustments,
        speed: PlaybackSpeed,
        segments: Vec<ClipSegment>,
        export_mode: SegmentExportMode,
        sender: async_channel::Sender<Result<(u64, u64), ()>>,
        running_flag: Arc<AtomicBool>,
    ) {
        std::thread::spawn(move || {
            let segments = segments
                .into_iter()
                .filter(|segment| segment.duration_ms() > 0)
                .collect::<Vec<_>>();
            if segments.is_empty() {
                let _ = sender.send_blocking(Err(()));
                return;
            }

            let total_ms = segments.iter().map(ClipSegment::duration_ms).sum::<u64>();
            let mut temps = TempFiles::default();
            let mut parts = Vec::with_capacity(segments.len());
            let ext = output_format.container_format.extension().to_owned();

            for (index, segment) in segments.iter().enumerate() {
                let part_path = if export_mode == SegmentExportMode::Join && segments.len() == 1 {
                    output_target.clone()
                } else {
                    match export_mode {
                        SegmentExportMode::Join => {
                            temps.reserve(&output_target, &format!("copy-section-{index}"), &ext)
                        }
                        SegmentExportMode::Separate => {
                            unique_segment_path(&output_target, segment.source_path(), index, &ext)
                        }
                    }
                };

                let mut command = ffmpeg_base();
                command.args([
                    "-ss",
                    &format!("{:.3}", segment.range.start_ms as f64 / 1_000.0),
                ]);
                command.arg("-i").arg(segment.source_path());
                command.args([
                    "-t",
                    &format!("{:.3}", segment.duration_ms() as f64 / 1_000.0),
                    "-map",
                    "0:v:0",
                ]);
                if plan.audio {
                    command.args(["-map", "0:a:0"]);
                }
                command.args(["-c", "copy", "-avoid_negative_ts", "make_zero"]);
                command.arg(&part_path);

                match run_ffmpeg_stage(
                    &mut command,
                    segment.duration_ms(),
                    segments[..index].iter().map(ClipSegment::duration_ms).sum(),
                    segment.duration_ms(),
                    total_ms.max(1),
                    &sender,
                    &running_flag,
                ) {
                    StageOutcome::Done => parts.push(part_path),
                    StageOutcome::Cancelled => {
                        let _ = sender.send_blocking(Err(()));
                        return;
                    }
                    StageOutcome::Failed => {
                        log::warn!("stream copy failed; falling back to recoding");
                        Self::save_ffmpeg_segments(
                            output_target.clone(),
                            output_format.clone(),
                            framerate,
                            oriented,
                            crop,
                            scaled,
                            orientation,
                            mute,
                            prefer_gpu,
                            adjustments,
                            speed,
                            segments.clone(),
                            export_mode,
                            sender.clone(),
                            running_flag.clone(),
                        );
                        return;
                    }
                }
            }

            if export_mode == SegmentExportMode::Separate || parts.len() == 1 {
                let _ = sender.send_blocking(Ok((total_ms, total_ms)));
                return;
            }

            let list_path = temps.reserve(&output_target, "copy-sections", "txt");
            let list = parts
                .iter()
                .map(|part| concat_entry(part))
                .collect::<String>();
            if let Err(err) = std::fs::File::create(&list_path)
                .and_then(|mut file| file.write_all(list.as_bytes()))
            {
                log::warn!("could not write copy section list ({err}); falling back to recoding");
                Self::save_ffmpeg_segments(
                    output_target,
                    output_format,
                    framerate,
                    oriented,
                    crop,
                    scaled,
                    orientation,
                    mute,
                    prefer_gpu,
                    adjustments,
                    speed,
                    segments,
                    export_mode,
                    sender,
                    running_flag,
                );
                return;
            }

            let mut command = ffmpeg_base();
            command.args(["-f", "concat", "-safe", "0", "-i"]);
            command.arg(&list_path);
            command.args(["-c", "copy", "-avoid_negative_ts", "make_zero"]);
            command.arg(&output_target);
            match run_ffmpeg_stage(
                &mut command,
                total_ms,
                total_ms.saturating_sub(1),
                1,
                total_ms.max(1),
                &sender,
                &running_flag,
            ) {
                StageOutcome::Done => {
                    let _ = sender.send_blocking(Ok((total_ms, total_ms)));
                }
                StageOutcome::Cancelled => {
                    let _ = sender.send_blocking(Err(()));
                }
                StageOutcome::Failed => {
                    log::warn!("stream-copy concat failed; falling back to recoding");
                    Self::save_ffmpeg_segments(
                        output_target,
                        output_format,
                        framerate,
                        oriented,
                        crop,
                        scaled,
                        orientation,
                        mute,
                        prefer_gpu,
                        adjustments,
                        speed,
                        segments,
                        export_mode,
                        sender,
                        running_flag,
                    );
                }
            }
        });
    }

    /// Renders each source range serially. Joined exports concatenate the encoded
    /// parts by stream copy; separate exports write one safely named file per range.
    #[allow(clippy::too_many_arguments)]
    fn save_ffmpeg_segments(
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
        segments: Vec<ClipSegment>,
        export_mode: SegmentExportMode,
        sender: async_channel::Sender<Result<(u64, u64), ()>>,
        running_flag: Arc<AtomicBool>,
    ) {
        std::thread::spawn(move || {
            let segments = segments
                .into_iter()
                .filter(|segment| segment.duration_ms() > 0)
                .collect::<Vec<_>>();
            if segments.is_empty() {
                let _ = sender.send_blocking(Err(()));
                return;
            }

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
            let audio_enabled = audio_codec.is_some()
                && segments.iter().any(|segment| {
                    get_info(segment.source.to_string_lossy().into_owned())
                        .map(|(_, _, has_audio)| has_audio)
                        .unwrap_or(false)
                });
            let ext = output_format.container_format.extension().to_owned();
            let source_ms = speed
                .output_duration_ms(segments.iter().map(ClipSegment::duration_ms).sum::<u64>());
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

            for (index, segment) in segments.iter().enumerate() {
                let range = segment.range;
                let source_dimensions = get_info(segment.source.to_string_lossy().into_owned())
                    .map(|(dimensions, _, _)| {
                        let dimensions: Dimensions<f64> = dimensions.into();
                        if orientation.is_width_height_swapped() {
                            dimensions.swap()
                        } else {
                            dimensions
                        }
                    })
                    .unwrap_or(oriented);
                let filters = ffmpeg_filter_chain(
                    source_dimensions,
                    crop,
                    scaled,
                    orientation,
                    adjustments,
                    speed,
                    framerate,
                );
                let part_path = match export_mode {
                    SegmentExportMode::Join => {
                        temps.reserve(&output_target, &format!("section-{index}"), &ext)
                    }
                    SegmentExportMode::Separate => {
                        unique_segment_path(&output_target, segment.source_path(), index, &ext)
                    }
                };

                let mut cmd = ffmpeg_base();
                cmd.args([
                    "-ss",
                    &format!("{:.3}", range.start_ms as f64 / 1000.0),
                    "-t",
                    &format!("{:.3}", range.duration_ms() as f64 / 1000.0),
                ]);
                cmd.arg("-i").arg(segment.source_path());
                let source_has_audio = get_info(segment.source.to_string_lossy().into_owned())
                    .map(|(_, _, has_audio)| has_audio)
                    .unwrap_or(false);
                cmd.args(["-map", "0:v:0"]);
                if audio_enabled {
                    if source_has_audio {
                        cmd.args(["-map", "0:a:0"]);
                    } else {
                        // Keep every joined part's stream layout identical.  A
                        // silent source gets a finite audio input; -shortest
                        // below cuts it to the rendered video duration.
                        cmd.args([
                            "-f",
                            "lavfi",
                            "-i",
                            "anullsrc=channel_layout=stereo:sample_rate=48000",
                            "-map",
                            "1:a:0",
                        ]);
                    }
                }
                cmd.args(["-vf", &filters.join(",")]);
                cmd.arg("-c:v").arg(encoder);
                cmd.args(&quality_args);
                match (audio_enabled, audio_codec) {
                    (true, Some(codec)) => {
                        let audio_filter = ffmpeg_audio_filter(speed);
                        cmd.arg("-af").arg(audio_filter);
                        cmd.arg("-c:a").arg(codec);
                        cmd.args(["-ar", "48000", "-ac", "2", "-shortest"]);
                    }
                    (false, _) => {
                        cmd.arg("-an");
                    }
                    (true, None) => {
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
                cmd.args(["-map", "0:v:0", "-map", "0:a:0?"]);
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

/// Audio layout shared by every rendered part in a joined sequence.  The
/// concat demuxer requires matching stream parameters, so normalise real and
/// synthetic audio before encoding rather than relying on each source's
/// channel count and sample rate.
fn ffmpeg_audio_filter(speed: PlaybackSpeed) -> String {
    let mut filters = vec!["aformat=sample_rates=48000:channel_layouts=stereo".to_owned()];
    if let Some(filter) = speed.ffmpeg_audio_filter() {
        filters.push(filter.to_owned());
    }
    filters.join(",")
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
