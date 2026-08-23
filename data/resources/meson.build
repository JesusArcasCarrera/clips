blueprints = custom_target('blueprints',
  input: files(
    'blueprints/help-overlay.blp',
    'blueprints/window.blp',
    'blueprints/timeline.blp',
    'blueprints/crop.blp',
    'blueprints/video-preview.blp',
  ),
  output: '.',
  command: [find_program('blueprint-compiler'), 'batch-compile', '@OUTPUT@', '@CURRENT_SOURCE_DIR@', '@INPUT@'],
)

scalable_dir = join_paths('icons', 'hicolor', 'scalable', 'apps')
install_data(
  join_paths(scalable_dir, ('@0@.svg').format(application_id)),
  install_dir: join_paths(get_option('datadir'), scalable_dir)
)
install_data(
  join_paths(scalable_dir, ('@0@.svg').format(base_id)),
  install_dir: join_paths(get_option('datadir'), scalable_dir)
)

message(join_paths(pkgdatadir, 'icons', scalable_dir))

symbolic_dir = join_paths('icons', 'hicolor', 'symbolic', 'apps')
install_data(
  join_paths(symbolic_dir, ('@0@-symbolic.svg').format(base_id)),
  install_dir: join_paths(pkgdatadir, symbolic_dir)
)

action_dir = join_paths('icons', 'hicolor', 'scalable', 'actions')
action_icons = [
    # each icon must be registered here
    join_paths(action_dir, 'image-missing-symbolic.svg'),
    join_paths(action_dir, 'paper-symbolic.svg'),
    join_paths(action_dir, 'usb-stick-symbolic.svg'),
    join_paths(action_dir, 'check-round-outline-symbolic.svg'),
    join_paths(action_dir, 'view-refresh-symbolic.svg'),
    join_paths(action_dir, 'error-symbolic.svg'),
    join_paths(action_dir, 'application-x-generic.svg'),
    join_paths(action_dir, 'play-symbolic.svg'),
    join_paths(action_dir, 'pause-symbolic.svg'),
    join_paths(action_dir, 'horizontal-flip-symbolic.svg'),
    join_paths(action_dir, 'vertical-flip-symbolic.svg'),
    join_paths(action_dir, 'chain-link-symbolic.svg'),
    join_paths(action_dir, 'chain-link-loose-symbolic.svg'),
]

install_data(
  action_icons,
  install_dir: join_paths(get_option('datadir'), action_dir)
)


# Resources
resources = gnome.compile_resources(
    'resources',
    'resources.gresource.xml',
    gresource_bundle: true,
    source_dir: meson.current_build_dir(),
    install: true,
    install_dir: pkgdatadir,
    dependencies: blueprints,
)
