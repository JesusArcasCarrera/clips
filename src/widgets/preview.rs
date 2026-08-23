use std::{
    cell::RefCell,
    path::PathBuf,
    sync::{atomic::AtomicBool, Arc},
};

use glib::clone;
use gst::{ClockTime, PadProbeData, PadProbeType, SeekFlags};
use gstreamer_pbutils::{Discoverer, ElementProperties, ElementPropertiesMapItem};
use gtk::{gdk, gio, glib, subclass::prelude::*};

use ges::prelude::*;
use ges::Effect;

use crate::{
    info::{get_info, Dimensions, Framerate},
    orientation::VideoOrientation,
    profiles::{ContainerFormat, OutputFormat, VideoEncoding},
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

        let op = self.imp().pipeline.borrow();

        if let Some(p) = op.as_ref() {
            p.seek_simple(SeekFlags::empty(), ClockTime::from_mseconds(position))
                .ok();
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

    pub fn save(
        &self,
        output_path: PathBuf,
        sender: async_channel::Sender<Result<(u64, u64), ()>>,
        output_format: OutputFormat,
        framerate: Framerate,
        scaled_dimension: Dimensions<u32>,
        running_flag: Arc<AtomicBool>,
    ) {
        self.kill();

        dbg!(&output_format, &framerate, &scaled_dimension);

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
                let video_caps =
                    gst::Caps::builder(video_encoding.get_format()).build();

                let video_profile_builder =
                    gstreamer_pbutils::EncodingVideoProfile::builder(&video_caps)
                        .preset_name(video_encoding.get_preset_name());

                let video_profile = if let Some(bitrate_kbps) = output_format.quality.bitrate_kbps() {
                    let (prop_name, multiplier) = video_encoding.bitrate_property();
                    let encoder_name = video_encoding.get_preset_name();
                    let bitrate_value = bitrate_kbps as i32 * multiplier as i32;
                    let props = ElementProperties::builder_map()
                        .item(
                            ElementPropertiesMapItem::builder(encoder_name)
                                .field(prop_name, bitrate_value)
                                .build(),
                        )
                        .build();
                    video_profile_builder.element_properties(props).build()
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
}
