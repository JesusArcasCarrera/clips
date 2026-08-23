// https://gitlab.gnome.org/YaLTeR/video-trimmer/-/blob/master/src/timeline.rs

use gtk::{gdk, prelude::*, subclass::prelude::*};
use gtk::{gio, glib};

mod imp {
    use super::*;
    use glib::{clone, subclass::Signal};
    use gtk::{
        gdk::{Key, RGBA},
        graphene, CompositeTemplate,
    };
    use itertools::Itertools;
    use once_cell::unsync::OnceCell;
    use ordered_float::NotNan;
    use std::cell::Cell;

    const TOLERANCE: f64 = 15.;
    const PIXEL_KEYBOARD_MOVE: f64 = 6.;

    #[derive(Debug, Clone, Copy, Eq, PartialEq)]
    enum DragType {
        Top,
        Right,
        Bottom,
        Left,
        TopRight,
        BottomRight,
        BottomLeft,
        TopLeft,
        All,
    }

    #[derive(Debug, Clone, Copy, Eq, PartialEq)]
    enum CursorType {
        Normal,
        Top,
        Bottom,
        Left,
        Right,
        TopRight,
        BottomRight,
        BottomLeft,
        TopLeft,
        All,
    }

    impl CursorType {
        fn gtk_cursor_name(self) -> &'static str {
            match self {
                CursorType::Normal => "default",
                CursorType::Top => "n-resize",
                CursorType::Bottom => "s-resize",
                CursorType::Left => "w-resize",
                CursorType::Right => "e-resize",
                CursorType::TopRight => "ne-resize",
                CursorType::BottomLeft => "sw-resize",
                CursorType::TopLeft => "nw-resize",
                CursorType::BottomRight => "se-resize",
                CursorType::All => "move",
            }
        }
    }

    #[derive(Debug, CompositeTemplate)]
    #[template(resource = "/io/gitlab/adhami3310/Clips/blueprints/crop.ui")]
    pub struct Crop {
        #[template_child]
        pub crop_box: TemplateChild<gtk::Box>,
        #[template_child]
        pub inner_crop_box: TemplateChild<gtk::Box>,
        #[template_child]
        pub top: TemplateChild<gtk::Box>,
        #[template_child]
        pub bottom: TemplateChild<gtk::Box>,
        #[template_child]
        pub container: TemplateChild<gtk::Box>,
        #[template_child]
        pub top_left: TemplateChild<gtk::Button>,
        #[template_child]
        pub top_right: TemplateChild<gtk::Button>,
        #[template_child]
        pub bottom_left: TemplateChild<gtk::Button>,
        #[template_child]
        pub bottom_right: TemplateChild<gtk::Button>,

        gesture_drag: OnceCell<gtk::GestureDrag>,
        drag_start: Cell<(f64, f64, f64, f64)>,
        pub current_selection: Cell<(f64, f64, f64, f64)>,
        drag_type: Cell<Option<DragType>>,
        cursor_type: Cell<CursorType>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for Crop {
        const NAME: &'static str = "Crop";
        type Type = super::Crop;
        type ParentType = gtk::Widget;

        fn class_init(klass: &mut Self::Class) {
            Self::bind_template(klass);

            klass.set_css_name("cropbox");
        }

        fn instance_init(obj: &glib::subclass::InitializingObject<Self>) {
            obj.init_template();
        }

        fn new() -> Self {
            Self {
                crop_box: TemplateChild::default(),
                container: TemplateChild::default(),
                inner_crop_box: TemplateChild::default(),
                top: TemplateChild::default(),
                bottom: TemplateChild::default(),
                top_left: TemplateChild::default(),
                top_right: TemplateChild::default(),
                bottom_left: TemplateChild::default(),
                bottom_right: TemplateChild::default(),

                gesture_drag: OnceCell::new(),
                drag_start: Cell::new((0., 0., 0., 0.)),
                current_selection: Cell::new((0., 0., 0., 0.)),
                drag_type: Cell::new(None),
                cursor_type: Cell::new(CursorType::Normal),
            }
        }
    }

    impl ObjectImpl for Crop {
        fn constructed(&self) {
            let obj = self.obj();
            self.parent_constructed();
            // Set up the drag gesture.
            let gesture_drag = gtk::GestureDrag::new();
            gesture_drag.connect_drag_begin({
                let obj = obj.downgrade();
                move |_, x, y| {
                    let obj = obj.upgrade().unwrap();
                    let imp = obj.imp();
                    imp.on_drag_start(x, y);
                }
            });
            gesture_drag.connect_drag_update({
                let obj = obj.downgrade();
                move |_, offset_x, offset_y| {
                    let obj = obj.upgrade().unwrap();
                    let imp = obj.imp();
                    imp.on_drag_update(offset_x, offset_y);
                }
            });
            gesture_drag.connect_drag_end({
                let obj = obj.downgrade();
                move |_, _, _| {
                    let obj = obj.upgrade().unwrap();
                    let imp = obj.imp();
                    imp.on_drag_end();
                }
            });
            obj.add_controller(gesture_drag.clone());
            self.gesture_drag.set(gesture_drag).unwrap();

            let event_controller_motion = gtk::EventControllerMotion::new();
            event_controller_motion.connect_motion({
                let obj = obj.downgrade();
                move |_, x, y| {
                    let obj = obj.upgrade().unwrap();
                    let imp = obj.imp();
                    imp.on_motion(x, y);
                }
            });
            obj.add_controller(event_controller_motion);

            let event_controller_keyboard = gtk::EventControllerKey::new();
            event_controller_keyboard.connect_key_pressed(clone!(
                #[weak(rename_to=this)]
                self,
                #[upgrade_or]
                glib::Propagation::Stop,
                move |_, k, _, _| {
                    match k {
                        Key::Down => {
                            this.bring_top_down();
                            glib::Propagation::Stop
                        }
                        Key::Up => {
                            this.bring_top_up();
                            glib::Propagation::Stop
                        }
                        Key::Left => {
                            this.bring_left_left();
                            glib::Propagation::Stop
                        }
                        Key::Right => {
                            this.bring_left_right();
                            glib::Propagation::Stop
                        }
                        _ => glib::Propagation::Proceed,
                    }
                }
            ));
            self.top_left.add_controller(event_controller_keyboard);

            let event_controller_keyboard = gtk::EventControllerKey::new();
            event_controller_keyboard.connect_key_pressed(clone!(
                #[weak(rename_to = this)]
                self,
                #[upgrade_or]
                glib::Propagation::Stop,
                move |_, k, _, _| {
                    match k {
                        Key::Down => {
                            this.bring_bottom_down();
                            glib::Propagation::Stop
                        }
                        Key::Up => {
                            this.bring_bottom_up();
                            glib::Propagation::Stop
                        }
                        Key::Left => {
                            this.bring_left_left();
                            glib::Propagation::Stop
                        }
                        Key::Right => {
                            this.bring_left_right();
                            glib::Propagation::Stop
                        }
                        _ => glib::Propagation::Proceed,
                    }
                }
            ));
            self.bottom_left.add_controller(event_controller_keyboard);

            let event_controller_keyboard = gtk::EventControllerKey::new();
            event_controller_keyboard.connect_key_pressed(clone!(
                #[weak(rename_to=this)]
                self,
                #[upgrade_or]
                glib::Propagation::Stop,
                move |_, k, _, _| {
                    match k {
                        Key::Down => {
                            this.bring_top_down();
                            glib::Propagation::Stop
                        }
                        Key::Up => {
                            this.bring_top_up();
                            glib::Propagation::Stop
                        }
                        Key::Left => {
                            this.bring_right_left();
                            glib::Propagation::Stop
                        }
                        Key::Right => {
                            this.bring_right_right();
                            glib::Propagation::Stop
                        }
                        _ => glib::Propagation::Proceed,
                    }
                }
            ));
            self.top_right.add_controller(event_controller_keyboard);

            let event_controller_keyboard = gtk::EventControllerKey::new();
            event_controller_keyboard.connect_key_pressed(clone!(
                #[weak(rename_to = this)]
                self,
                #[upgrade_or]
                glib::Propagation::Stop,
                move |_, k, _, _| {
                    match k {
                        Key::Down => {
                            this.bring_bottom_down();
                            glib::Propagation::Stop
                        }
                        Key::Up => {
                            this.bring_bottom_up();
                            glib::Propagation::Stop
                        }
                        Key::Left => {
                            this.bring_right_left();
                            glib::Propagation::Stop
                        }
                        Key::Right => {
                            this.bring_right_right();
                            glib::Propagation::Stop
                        }
                        _ => glib::Propagation::Proceed,
                    }
                }
            ));
            self.bottom_right.add_controller(event_controller_keyboard);
        }

        fn signals() -> &'static [Signal] {
            use once_cell::sync::Lazy;
            static SIGNALS: Lazy<[Signal; 1]> = Lazy::new(|| {
                [Signal::builder("crop-box-changed")
                    .param_types([
                        glib::Type::F64,
                        glib::Type::F64,
                        glib::Type::F64,
                        glib::Type::F64,
                    ])
                    .build()]
            });

            SIGNALS.as_ref()
        }

        fn dispose(&self) {
            let obj = self.obj();
            while let Some(child) = obj.first_child() {
                child.unparent();
            }
        }
    }

    impl WidgetImpl for Crop {
        fn size_allocate(&self, width: i32, height: i32, baseline: i32) {
            let crop = self.current_selection.get();

            self.container.size_allocate(
                &gtk::Allocation::new(
                    (width as f64 * crop.3).round() as i32,
                    (height as f64 * crop.0).round() as i32,
                    (width as f64 * (1. - crop.3 - crop.1)).round() as i32,
                    (height as f64 * (1. - crop.2 - crop.0)).round() as i32,
                ),
                baseline,
            );
        }

        fn snapshot(&self, snapshot: &gtk::Snapshot) {
            let c = RGBA::builder()
                .red(0.)
                .green(0.)
                .blue(0.)
                .alpha(0.5)
                .build();

            let (width, height) = (self.obj().width() as f32, self.obj().height() as f32);

            let crop = self.current_selection.get();

            let top = (height as f64 * crop.0).round() as f32;
            let left = (width as f64 * crop.3).round() as f32;
            let bottom = top + (height as f64 * (1. - crop.2 - crop.0)).round() as f32;
            let right = left + (width as f64 * (1. - crop.3 - crop.1)).round() as f32;

            snapshot.append_color(&c, &graphene::Rect::new(0., 0., width, top));
            snapshot.append_color(&c, &graphene::Rect::new(0., top, left, bottom - top));
            snapshot.append_color(&c, &graphene::Rect::new(0., bottom, width, height - bottom));
            snapshot.append_color(
                &c,
                &graphene::Rect::new(right, top, width - right, bottom - top),
            );

            self.obj()
                .snapshot_child(&self.obj().first_child().unwrap(), snapshot);
        }
    }

    impl Crop {
        fn positons(&self) -> (f64, f64, f64, f64) {
            let crop = self.current_selection.get();
            let (width, height) = (self.obj().width(), self.obj().height());
            (
                (height as f64 * crop.0),
                (width as f64 * (1. - crop.1)),
                (height as f64 * (1. - crop.2)),
                (width as f64 * crop.3),
            )
        }

        fn calculate_drag_type(&self, x: f64, y: f64) -> Option<DragType> {
            let (t, r, b, l) = self.positons();

            let (dt, dr, db, dl) = ((y - t).abs(), (x - r).abs(), (y - b).abs(), (x - l).abs());

            let ((i0, v0), (i1, v1)) = [dt, dr, db, dl]
                .into_iter()
                .flat_map(NotNan::new)
                .enumerate()
                .sorted_by_key(|(_, x)| *x)
                .take(2)
                .collect_tuple()
                .unwrap();

            if v0 > NotNan::new(TOLERANCE).unwrap() {
                if x >= l && x <= r && y >= t && y <= b {
                    return Some(DragType::All);
                } else {
                    return None;
                }
            }

            if v1 > NotNan::new(TOLERANCE).unwrap() {
                if v0 < NotNan::new(TOLERANCE).unwrap() {
                    let current_drag = Some(
                        [
                            DragType::Top,
                            DragType::Right,
                            DragType::Bottom,
                            DragType::Left,
                        ][i0],
                    );

                    return current_drag;
                }
                return None;
            }

            let current_drag = match (
                [
                    DragType::Top,
                    DragType::Right,
                    DragType::Bottom,
                    DragType::Left,
                ][i0],
                [
                    DragType::Top,
                    DragType::Right,
                    DragType::Bottom,
                    DragType::Left,
                ][i1],
            ) {
                (DragType::Top, DragType::Left) | (DragType::Left, DragType::Top) => {
                    DragType::TopLeft
                }
                (DragType::Top, DragType::Right) | (DragType::Right, DragType::Top) => {
                    DragType::TopRight
                }
                (DragType::Bottom, DragType::Left) | (DragType::Left, DragType::Bottom) => {
                    DragType::BottomLeft
                }
                (DragType::Bottom, DragType::Right) | (DragType::Right, DragType::Bottom) => {
                    DragType::BottomRight
                }
                (x, _) => x,
            };

            Some(current_drag)
        }

        fn on_drag_start(&self, x: f64, y: f64) {
            let drag_type = self.calculate_drag_type(x, y);

            if drag_type.is_some() {
                self.drag_start.set(self.current_selection.get());
                self.drag_type.set(drag_type);
                self.on_drag_update(0., 0.);
            }
        }

        fn on_drag_update(&self, offset_x: f64, offset_y: f64) {
            if self.drag_type.get().is_none() {
                return;
            }

            let current_selection = self.current_selection.get();
            let old_selection = self.drag_start.get();

            let min_size = 0.05;
            let width = 1. - current_selection.1 - current_selection.3 - min_size;
            let height = 1. - current_selection.0 - current_selection.2 - min_size;

            let offset_x = offset_x / (self.obj().width() as f64);
            let offset_y = offset_y / (self.obj().height() as f64);

            let actual_offset_y = offset_y - (current_selection.0 - old_selection.0)
                + (current_selection.2 - old_selection.2);
            let actual_offset_x = offset_x - (current_selection.3 - old_selection.3)
                + (current_selection.1 - old_selection.1);

            let drag_type = self.drag_type.get().unwrap();

            if matches!(drag_type, DragType::All) {
                let actual_offset_y = offset_y - (current_selection.0 - old_selection.0);
                let actual_offset_x = offset_x - (current_selection.3 - old_selection.3);
                let offset_y = actual_offset_y
                    .clamp(-current_selection.0, height)
                    .clamp(-height, current_selection.2);
                let offset_x = actual_offset_x
                    .clamp(-current_selection.3, width)
                    .clamp(-width, current_selection.1);

                self.current_selection.set((
                    offset_y + current_selection.0,
                    -offset_x + current_selection.1,
                    -offset_y + current_selection.2,
                    offset_x + current_selection.3,
                ));
            }

            if matches!(
                drag_type,
                DragType::Top | DragType::TopLeft | DragType::TopRight
            ) {
                let offset_y = actual_offset_y.clamp(-current_selection.0, height);

                let current_selection = self.current_selection.get();

                self.current_selection.set((
                    offset_y + current_selection.0,
                    current_selection.1,
                    current_selection.2,
                    current_selection.3,
                ));
            }
            if matches!(
                drag_type,
                DragType::Bottom | DragType::BottomLeft | DragType::BottomRight
            ) {
                let offset_y = actual_offset_y.clamp(-height, current_selection.2);

                let current_selection = self.current_selection.get();

                self.current_selection.set((
                    current_selection.0,
                    current_selection.1,
                    -offset_y + current_selection.2,
                    current_selection.3,
                ));
            }
            if matches!(
                drag_type,
                DragType::Left | DragType::BottomLeft | DragType::TopLeft
            ) {
                let offset_x = actual_offset_x.clamp(-current_selection.3, width);

                let current_selection = self.current_selection.get();

                self.current_selection.set((
                    current_selection.0,
                    current_selection.1,
                    current_selection.2,
                    offset_x + current_selection.3,
                ));
            }
            if matches!(
                drag_type,
                DragType::Right | DragType::BottomRight | DragType::TopRight
            ) {
                let offset_x = actual_offset_x.clamp(-width, current_selection.1);

                let current_selection = self.current_selection.get();

                self.current_selection.set((
                    current_selection.0,
                    -offset_x + current_selection.1,
                    current_selection.2,
                    current_selection.3,
                ));
            }

            let current_selection = self.current_selection.get();

            self.obj().emit_by_name::<()>(
                "crop-box-changed",
                &[
                    &current_selection.0,
                    &current_selection.1,
                    &current_selection.2,
                    &current_selection.3,
                ],
            );

            self.obj().queue_allocate();
        }

        fn on_drag_end(&self) {
            self.drag_type.set(None);
        }

        fn bring_top_down(&self) {
            let (_width, height) = (self.obj().width(), self.obj().height());

            let current_selection = self.current_selection.get();

            self.current_selection.set((
                (current_selection.0 + PIXEL_KEYBOARD_MOVE / (height as f64))
                    .clamp(0., 1. - current_selection.2),
                current_selection.1,
                current_selection.2,
                current_selection.3,
            ));

            let current_selection = self.current_selection.get();

            self.obj().emit_by_name::<()>(
                "crop-box-changed",
                &[
                    &current_selection.0,
                    &current_selection.1,
                    &current_selection.2,
                    &current_selection.3,
                ],
            );

            self.obj().queue_allocate();
        }

        fn bring_top_up(&self) {
            let (_width, height) = (self.obj().width(), self.obj().height());

            let current_selection = self.current_selection.get();

            self.current_selection.set((
                (current_selection.0 - PIXEL_KEYBOARD_MOVE / (height as f64))
                    .clamp(0., 1. - current_selection.2),
                current_selection.1,
                current_selection.2,
                current_selection.3,
            ));

            let current_selection = self.current_selection.get();

            self.obj().emit_by_name::<()>(
                "crop-box-changed",
                &[
                    &current_selection.0,
                    &current_selection.1,
                    &current_selection.2,
                    &current_selection.3,
                ],
            );

            self.obj().queue_allocate();
        }

        fn bring_bottom_down(&self) {
            let (_width, height) = (self.obj().width(), self.obj().height());

            let current_selection = self.current_selection.get();

            self.current_selection.set((
                current_selection.0,
                current_selection.1,
                (current_selection.2 - PIXEL_KEYBOARD_MOVE / (height as f64))
                    .clamp(0., 1. - current_selection.0),
                current_selection.3,
            ));

            let current_selection = self.current_selection.get();

            self.obj().emit_by_name::<()>(
                "crop-box-changed",
                &[
                    &current_selection.0,
                    &current_selection.1,
                    &current_selection.2,
                    &current_selection.3,
                ],
            );

            self.obj().queue_allocate();
        }

        fn bring_bottom_up(&self) {
            let (_width, height) = (self.obj().width(), self.obj().height());

            let current_selection = self.current_selection.get();

            self.current_selection.set((
                current_selection.0,
                current_selection.1,
                (current_selection.2 + PIXEL_KEYBOARD_MOVE / (height as f64))
                    .clamp(0., 1. - current_selection.0),
                current_selection.3,
            ));

            let current_selection = self.current_selection.get();

            self.obj().emit_by_name::<()>(
                "crop-box-changed",
                &[
                    &current_selection.0,
                    &current_selection.1,
                    &current_selection.2,
                    &current_selection.3,
                ],
            );

            self.obj().queue_allocate();
        }

        fn bring_left_right(&self) {
            let (width, _height) = (self.obj().width(), self.obj().height());

            let current_selection = self.current_selection.get();

            self.current_selection.set((
                current_selection.0,
                current_selection.1,
                current_selection.2,
                (current_selection.3 + PIXEL_KEYBOARD_MOVE / (width as f64))
                    .clamp(0., 1. - current_selection.1),
            ));

            let current_selection = self.current_selection.get();

            self.obj().emit_by_name::<()>(
                "crop-box-changed",
                &[
                    &current_selection.0,
                    &current_selection.1,
                    &current_selection.2,
                    &current_selection.3,
                ],
            );

            self.obj().queue_allocate();
        }

        fn bring_left_left(&self) {
            let (width, _height) = (self.obj().width(), self.obj().height());

            let current_selection = self.current_selection.get();

            self.current_selection.set((
                current_selection.0,
                current_selection.1,
                current_selection.2,
                (current_selection.3 - PIXEL_KEYBOARD_MOVE / (width as f64))
                    .clamp(0., 1. - current_selection.1),
            ));

            let current_selection = self.current_selection.get();

            self.obj().emit_by_name::<()>(
                "crop-box-changed",
                &[
                    &current_selection.0,
                    &current_selection.1,
                    &current_selection.2,
                    &current_selection.3,
                ],
            );

            self.obj().queue_allocate();
        }

        fn bring_right_right(&self) {
            let (width, _height) = (self.obj().width(), self.obj().height());

            let current_selection = self.current_selection.get();

            self.current_selection.set((
                current_selection.0,
                (current_selection.1 - PIXEL_KEYBOARD_MOVE / (width as f64))
                    .clamp(0., 1. - current_selection.3),
                current_selection.2,
                current_selection.3,
            ));

            let current_selection = self.current_selection.get();

            self.obj().emit_by_name::<()>(
                "crop-box-changed",
                &[
                    &current_selection.0,
                    &current_selection.1,
                    &current_selection.2,
                    &current_selection.3,
                ],
            );

            self.obj().queue_allocate();
        }

        fn bring_right_left(&self) {
            let (width, _height) = (self.obj().width(), self.obj().height());

            let current_selection = self.current_selection.get();

            self.current_selection.set((
                current_selection.0,
                (current_selection.1 + PIXEL_KEYBOARD_MOVE / (width as f64))
                    .clamp(0., 1. - current_selection.3),
                current_selection.2,
                current_selection.3,
            ));

            let current_selection = self.current_selection.get();

            self.obj().emit_by_name::<()>(
                "crop-box-changed",
                &[
                    &current_selection.0,
                    &current_selection.1,
                    &current_selection.2,
                    &current_selection.3,
                ],
            );

            self.obj().queue_allocate();
        }

        fn on_motion(&self, x: f64, y: f64) {
            let drag_type = self.calculate_drag_type(x, y);

            let cursor_type = match drag_type {
                Some(DragType::Top) => CursorType::Top,
                Some(DragType::Left) => CursorType::Left,
                Some(DragType::Bottom) => CursorType::Bottom,
                Some(DragType::Right) => CursorType::Right,
                Some(DragType::TopLeft) => CursorType::TopLeft,
                Some(DragType::BottomLeft) => CursorType::BottomLeft,
                Some(DragType::BottomRight) => CursorType::BottomRight,
                Some(DragType::TopRight) => CursorType::TopRight,
                Some(DragType::All) => CursorType::All,
                None => CursorType::Normal,
            };
            if self.cursor_type.get() != cursor_type {
                let cursor = gdk::Cursor::from_name(cursor_type.gtk_cursor_name(), None).unwrap();
                self.obj().set_cursor(Some(&cursor));
                self.cursor_type.set(cursor_type);
            }
        }
    }
}

glib::wrapper! {
    pub struct Crop(ObjectSubclass<imp::Crop>)
        @extends gtk::Widget,
        @implements gtk::Buildable, gtk::Accessible, gtk::ConstraintTarget, gio::ActionMap, gio::ActionGroup, gtk::Root;
}

impl Crop {
    pub fn proportions(&self) -> (f64, f64, f64, f64) {
        self.imp().current_selection.get()
    }

    pub fn set_proportions(&self, proportions: (f64, f64, f64, f64)) {
        self.imp().current_selection.set(proportions);
        self.emit_by_name::<()>(
            "crop-box-changed",
            &[
                &proportions.0,
                &proportions.1,
                &proportions.2,
                &proportions.3,
            ],
        );
        self.queue_allocate();
    }

    pub fn rotate_right_proportions(&self) -> (f64, f64, f64, f64) {
        let p = self.proportions();
        (p.3, p.0, p.1, p.2)
    }

    pub fn rotate_left_proportions(&self) -> (f64, f64, f64, f64) {
        let p = self.proportions();
        (p.1, p.2, p.3, p.0)
    }

    pub fn horizontal_flip_proportions(&self) -> (f64, f64, f64, f64) {
        let p = self.proportions();
        (p.0, p.3, p.2, p.3)
    }

    pub fn vertical_flip_proportions(&self) -> (f64, f64, f64, f64) {
        let p = self.proportions();
        (p.2, p.1, p.0, p.3)
    }

    pub fn reset(&self) {
        self.set_proportions((0.0, 0.0, 0.0, 0.0));
    }
}
