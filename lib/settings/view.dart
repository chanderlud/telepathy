import 'dart:async';
import 'dart:core';
import 'dart:io';
import 'package:flutter/foundation.dart' show kIsWeb;
import 'package:telepathy/settings/controller.dart';
import 'package:telepathy/src/rust/audio/player.dart';
import 'package:collection/collection.dart';
import 'package:file_picker/file_picker.dart';
import 'package:flutter/material.dart' hide Overlay;
import 'package:flutter/services.dart' show Clipboard, ClipboardData;
import 'package:flutter_colorpicker/flutter_colorpicker.dart';
import 'package:flutter_svg/flutter_svg.dart';
import 'package:telepathy/audio_level.dart';
import 'package:telepathy/console.dart';
import 'package:telepathy/main.dart';
import 'package:telepathy/src/rust/telepathy.dart';
import 'package:telepathy/src/rust/flutter.dart';
import 'package:telepathy/src/rust/error.dart';
import 'package:telepathy/src/rust/overlay/overlay.dart';
import 'package:telepathy/settings/logs.dart';

import 'menu.dart';

enum SettingsSection {
  audioVideo,
  profiles,
  networking,
  interface,
  logs,
  overlay,
}

class SettingsPage extends StatefulWidget {
  final SettingsController controller;
  final InterfaceController interfaceController;
  final Telepathy telepathy;
  final StateController stateController;
  final StatisticsController statisticsController;
  final SoundPlayer player;
  final Overlay overlay;
  final AudioDevices audioDevices;
  final BoxConstraints constraints;

  const SettingsPage(
      {super.key,
      required this.controller,
      required this.telepathy,
      required this.stateController,
      required this.player,
      required this.statisticsController,
      required this.overlay,
      required this.audioDevices,
      required this.constraints,
      required this.interfaceController});

  @override
  SettingsPageState createState() => SettingsPageState();
}

class SettingsPageState extends State<SettingsPage>
    with SingleTickerProviderStateMixin {
  SettingsSection _section = SettingsSection.audioVideo;
  int? hovered;
  bool? showMenu;

  final TextEditingController _searchController = TextEditingController();
  final GlobalKey<NetworkSettingsState> _key =
      GlobalKey<NetworkSettingsState>();

  late AnimationController _animationController;
  late Animation<Offset> _menuSlideAnimation;

  @override
  void initState() {
    super.initState();

    showMenu = widget.constraints.maxWidth > 600;

    _animationController = AnimationController(
      duration: const Duration(milliseconds: 100),
      vsync: this,
    );

    if (showMenu == false) {
      _animationController.value = 1;
    } else {
      _animationController.value = 0;
    }

    _menuSlideAnimation =
        Tween<Offset>(begin: const Offset(0, 0), end: const Offset(-1, 0))
            .animate(_animationController);
  }

  @override
  void dispose() {
    _animationController.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    BoxConstraints constraints = widget.constraints;
    double width;

    if (_section == SettingsSection.overlay) {
      width = 1000;
    } else if (_section == SettingsSection.logs) {
      width = 2000;
    } else {
      width = 650;
    }

    if (constraints.maxWidth > 600 && showMenu == false) {
      _animationController.reverse();
      showMenu = null;
    } else if (constraints.maxWidth > 600 && showMenu == true) {
      showMenu = null;
    } else if (constraints.maxWidth < 600 && showMenu == null) {
      _animationController.forward();
      showMenu = false;
    }

    return SafeArea(
        bottom: false,
        child: Stack(
          children: [
            Align(
              alignment: Alignment.topCenter,
              child: Padding(
                  padding: EdgeInsets.only(
                      left: constraints.maxWidth < 600 ? 0 : 200),
                  child: SingleChildScrollView(
                    child: Column(
                      children: [
                        Padding(
                          padding: EdgeInsets.only(
                              left: 20,
                              right: 20,
                              top: constraints.maxWidth < 600
                                  ? _section == SettingsSection.audioVideo
                                      ? 55
                                      : 70
                                  : _section == SettingsSection.audioVideo
                                      ? 10
                                      : 30),
                          child: SizedBox(
                            width: width,
                            child: LayoutBuilder(builder: (BuildContext context,
                                BoxConstraints constraints) {
                              if (_section == SettingsSection.audioVideo) {
                                return AVSettings(
                                  controller: widget.controller,
                                  telepathy: widget.telepathy,
                                  stateController: widget.stateController,
                                  player: widget.player,
                                  statisticsController:
                                      widget.statisticsController,
                                  constraints: constraints,
                                  audioDevices: widget.audioDevices,
                                );
                              } else if (_section == SettingsSection.profiles) {
                                return ProfileSettings(
                                    controller: widget.controller,
                                    telepathy: widget.telepathy,
                                    stateController: widget.stateController);
                              } else if (_section == SettingsSection.networking) {
                                return NetworkSettings(
                                    key: _key,
                                    controller: widget.controller,
                                    telepathy: widget.telepathy,
                                    stateController: widget.stateController,
                                    constraints: constraints);
                              } else if (_section == SettingsSection.interface) {
                                return InterfaceSettings(
                                    controller: widget.interfaceController,
                                    constraints: constraints);
                              } else if (_section == SettingsSection.logs) {
                                return LogsSettings(searchController: _searchController);
                              } else if (_section == SettingsSection.overlay) {
                                return OverlaySettings(
                                    overlay: widget.overlay,
                                    controller: widget.controller,
                                    stateController: widget.stateController);
                              } else {
                                return const SizedBox();
                              }
                            }),
                          ),
                        )
                      ],
                    ),
                  )),
            ),
            if (constraints.maxWidth > 600 || (showMenu ?? true))
              SlideTransition(
                position: _menuSlideAnimation,
                child: Container(
                  width: 200,
                  decoration: BoxDecoration(
                    color: Theme.of(context).colorScheme.surfaceDim,
                    borderRadius: const BorderRadius.only(
                      topRight: Radius.circular(8),
                      bottomRight: Radius.circular(8),
                    ),
                  ),
                  padding: const EdgeInsets.only(top: 60),
                  child: SettingsMenu(
                    selected: _section,
                    hoveredIndex: hovered,
                    onSectionSelected: (section) => tapHandler(section),
                    onHoverChanged: (idx, isHovered) => hoverHandler(idx, isHovered),
                    showOverlayItem: !kIsWeb && Platform.isWindows,
                  ),
                ),
              ),
            Align(
              alignment: Alignment.topLeft,
              child: Container(
                  padding: const EdgeInsets.only(left: 5, top: 5, bottom: 5),
                  decoration: BoxDecoration(
                    color: (showMenu ?? true)
                        ? null
                        : Theme.of(context).colorScheme.tertiaryContainer,
                    borderRadius: const BorderRadius.only(
                      bottomLeft: Radius.circular(8),
                      bottomRight: Radius.circular(8),
                    ),
                  ),
                  child: Row(
                    children: [
                      IconButton(
                        visualDensity: VisualDensity.comfortable,
                        icon: SvgPicture.asset(
                          'assets/icons/Back.svg',
                          semanticsLabel: 'Close Settings',
                          width: 30,
                        ),
                        onPressed: () async {
                          if (_section == SettingsSection.networking &&
                              (_key.currentState?.unsavedChanges ?? false)) {
                            bool leave = await unsavedConfirmation(context);

                            if (!leave) {
                              return;
                            }
                          }

                          if (context.mounted) {
                            Navigator.of(context).pop();
                          }
                        },
                      ),
                      const SizedBox(width: 3),
                      if (constraints.maxWidth < 600)
                        IconButton(
                          visualDensity: VisualDensity.comfortable,
                          icon: SvgPicture.asset(
                            showMenu ?? true
                                ? 'assets/icons/HamburgerOpened.svg'
                                : 'assets/icons/HamburgerClosed.svg',
                            semanticsLabel: 'Menu',
                            width: 30,
                          ),
                          onPressed: () {
                            setState(() {
                              if (showMenu ?? true) {
                                _animationController.forward();
                              } else {
                                _animationController.reverse();
                              }

                              showMenu = !(showMenu ?? true);
                            });
                          },
                        ),
                    ],
                  )),
            ),
          ],
        ));
  }

  Future<void> tapHandler(SettingsSection target) async {
    if (_section == SettingsSection.networking && (_key.currentState?.unsavedChanges ?? false)) {
      bool leave = await unsavedConfirmation(context);

      if (!leave) {
        return;
      }
    }

    setState(() {
      _section = target;
    });
  }

  void hoverHandler(int target, bool hovered) {
    setState(() {
      if (hovered) {
        this.hovered = target;
      } else {
        this.hovered = null;
      }
    });
  }

  Color getColor(SettingsSection target) {
    if (target == hovered) {
      return Theme.of(context).colorScheme.secondary;
    } else if (target == _section) {
      return Theme.of(context).colorScheme.primary;
    } else {
      return Theme.of(context).colorScheme.surfaceDim;
    }
  }
}

class AVSettings extends StatefulWidget {
  final SettingsController controller;
  final Telepathy telepathy;
  final StateController stateController;
  final StatisticsController statisticsController;
  final SoundPlayer player;
  final BoxConstraints constraints;
  final AudioDevices audioDevices;

  const AVSettings(
      {super.key,
      required this.controller,
      required this.telepathy,
      required this.stateController,
      required this.player,
      required this.statisticsController,
      required this.constraints,
      required this.audioDevices});

  @override
  State<StatefulWidget> createState() => _AVSettingsState();
}

class _AVSettingsState extends State<AVSettings> {
  Capabilities? _capabilities;
  RecordingConfig? _recordingConfig;
  TemporaryConfig? _temporaryConfig;
  bool _loading = false;

  @override
  void initState() {
    super.initState();
    widget.audioDevices.startUpdates();

    var capabilitiesFuture = widget.controller.screenshareConfig.capabilities();
    var recordingConfigFuture =
        widget.controller.screenshareConfig.recordingConfig();

    Future.wait([capabilitiesFuture, recordingConfigFuture])
        .then((List<dynamic> results) {
      _capabilities = results[0];
      _recordingConfig = results[1];
      setState(() {});
    });
  }

  @override
  void activate() {
    super.activate();
    widget.audioDevices.startUpdates();
  }

  @override
  void deactivate() {
    widget.audioDevices.pauseUpdates();
    super.deactivate();
  }

  void initTemporaryConfig(
      String? encoder, String? device, int? bitrate, int? framerate) {
    setState(() {
      _temporaryConfig = TemporaryConfig(
          encoder: encoder ?? defaultEncoder(),
          device: device ?? defaultDevice(),
          bitrate: bitrate ?? defaultBitrate(),
          framerate: framerate ?? defaultFramerate(),
          height: _recordingConfig?.height());
    });
  }

  String defaultEncoder() {
    return _recordingConfig?.encoder() ??
        _capabilities?.encoders().firstOrNull ??
        'h264';
  }

  String defaultDevice() {
    return _recordingConfig?.device() ?? _capabilities!.devices().first;
  }

  int defaultBitrate() {
    return _recordingConfig?.bitrate() ?? 2000000;
  }

  int defaultFramerate() {
    return _recordingConfig?.framerate() ?? 30;
  }

  @override
  Widget build(BuildContext context) {
    var encoders = _capabilities?.encoders() ?? [];
    var devices = _capabilities?.devices() ?? [];

    double width = widget.constraints.maxWidth < 650
        ? widget.constraints.maxWidth
        : (widget.constraints.maxWidth - 20) / 2;

    return Column(
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        const Text(
          'Audio Options',
          style: TextStyle(fontSize: 20),
        ),
        const SizedBox(height: 17),
        ListenableBuilder(
            listenable: widget.stateController,
            builder: (BuildContext context, Widget? child) {
              return ListenableBuilder(
                  listenable: widget.audioDevices,
                  builder: (BuildContext context, Widget? child) {
                    String inputInitialSelection;

                    if (widget.controller.inputDevice == null) {
                      inputInitialSelection = 'Default';
                    } else if (widget.audioDevices.inputDevices
                        .contains(widget.controller.inputDevice)) {
                      inputInitialSelection = widget.controller.inputDevice!;
                    } else {
                      inputInitialSelection = 'Default';
                    }

                    String outputInitialSelection;

                    if (widget.controller.outputDevice == null) {
                      outputInitialSelection = 'Default';
                    } else if (widget.audioDevices.outputDevices
                        .contains(widget.controller.outputDevice)) {
                      outputInitialSelection = widget.controller.outputDevice!;
                    } else {
                      outputInitialSelection = 'Default';
                    }

                    double width = widget.constraints.maxWidth < 650
                        ? widget.constraints.maxWidth
                        : (widget.constraints.maxWidth - 20) / 2;

                    return Wrap(
                      spacing: 20,
                      runSpacing: 20,
                      children: [
                        DropDown(
                            label: 'Input Device',
                            items: widget.audioDevices.inputDevices
                                .map((d) => (d, d))
                                .toList(),
                            initialSelection: inputInitialSelection,
                            onSelected: (String? value) {
                              if (value == 'Default') value = null;
                              widget.controller.updateInputDevice(value);
                              widget.telepathy.setInputDevice(device: value);
                            },
                            width: width),
                        DropDown(
                          label: 'Output Device',
                          items: widget.audioDevices.outputDevices
                              .map((d) => (d, d))
                              .toList(),
                          initialSelection: outputInitialSelection,
                          onSelected: (String? value) {
                            if (value == 'Default') value = null;
                            widget.controller.updateOutputDevice(value);
                            widget.telepathy.setOutputDevice(device: value);
                            widget.player.updateOutputDevice(name: value);
                          },
                          width: width,
                        )
                      ],
                    );
                  });
            }),
        const SizedBox(height: 20),
        Row(children: [
          ListenableBuilder(
              listenable: widget.stateController,
              builder: (BuildContext context, Widget? child) {
                return Button(
                  text: widget.stateController.inAudioTest
                      ? 'End Test'
                      : 'Sound Test',
                  width: 80,
                  height: 25,
                  disabled: widget.stateController.isCallActive,
                  onPressed: () async {
                    if (widget.stateController.inAudioTest) {
                      widget.stateController.setInAudioTest();
                      widget.telepathy.endCall();
                    } else {
                      widget.stateController.setInAudioTest();
                      try {
                        await widget.telepathy.audioTest();
                      } on DartError catch (e) {
                        if (!context.mounted) return;
                        showErrorDialog(
                            context, 'Error in Audio Test', e.message);
                        widget.stateController.setInAudioTest();
                      }
                    }
                  },
                );
              }),
          const SizedBox(width: 20),
          ListenableBuilder(
              listenable: widget.statisticsController,
              builder: (BuildContext context, Widget? child) {
                return AudioLevel(
                    level: widget.statisticsController.inputLevel,
                    numRectangles: (widget.constraints.maxWidth - 145) ~/ 13.5);
              }),
        ]),
        const SizedBox(height: 20),
        Row(
          mainAxisAlignment: MainAxisAlignment.spaceBetween,
          mainAxisSize: MainAxisSize.max,
          children: [
            const Text('Noise Suppression', style: TextStyle(fontSize: 18)),
            ListenableBuilder(
                listenable: widget.controller,
                builder: (BuildContext context, Widget? child) {
                  return ListenableBuilder(
                      listenable: widget.stateController,
                      builder: (BuildContext context, Widget? child) {
                        return DropDown(
                            items: const [
                              ('Off', 'Off'),
                              ('Vanilla', 'Vanilla'),
                              ('Hogwash', 'Hogwash')
                            ],
                            initialSelection: widget.controller.useDenoise
                                ? widget.controller.denoiseModel ?? 'Vanilla'
                                : 'Off',
                            onSelected: (String? value) {
                              if (value == 'Off') {
                                // save denoise option
                                widget.controller.updateUseDenoise(false);
                                // set denoise to false
                                widget.telepathy.setDenoise(denoise: false);
                              } else {
                                if (value == 'Vanilla') {
                                  value = null;
                                }

                                // save denoise option
                                widget.controller.updateUseDenoise(true);
                                // save denoise model
                                widget.controller.setDenoiseModel(value);
                                // set denoise to true
                                widget.telepathy.setDenoise(denoise: true);
                                // set denoise model
                                updateDenoiseModel(value, widget.telepathy);
                              }
                            });
                      });
                }),
          ],
        ),
        const SizedBox(height: 5),
        Row(
          mainAxisAlignment: MainAxisAlignment.spaceBetween,
          mainAxisSize: MainAxisSize.max,
          children: [
            const Text('Play Custom Ringtones', style: TextStyle(fontSize: 18)),
            ListenableBuilder(
                listenable: widget.controller,
                builder: (BuildContext context, Widget? child) {
                  return CustomSwitch(
                      value: widget.controller.playCustomRingtones,
                      onChanged: (play) {
                        widget.controller.updatePlayCustomRingtones(play);
                        widget.telepathy.setPlayCustomRingtones(play: play);
                      });
                }),
          ],
        ),
        const SizedBox(height: 15),
        Row(
          mainAxisAlignment: MainAxisAlignment.spaceBetween,
          mainAxisSize: MainAxisSize.max,
          children: [
            Button(
                text: 'Select custom ringtone file',
                onPressed: () async {
                  FilePickerResult? result =
                      await FilePicker.platform.pickFiles(
                    type: FileType.custom,
                    allowedExtensions: ['wav'],
                  );

                  if (result != null) {
                    String? path = result.files.single.path;
                    widget.controller.updateCustomRingtoneFile(path);
                    widget.telepathy.setSendCustomRingtone(send: true);
                    loadRingtone(path: path!);
                  } else {
                    widget.controller.updateCustomRingtoneFile(null);
                    widget.telepathy.setSendCustomRingtone(send: false);
                  }
                }),
            ListenableBuilder(
                listenable: widget.controller,
                builder: (BuildContext context, Widget? child) {
                  return Text(widget.controller.customRingtoneFile ?? '',
                      style: const TextStyle(fontSize: 16));
                }),
          ],
        ),
        const SizedBox(height: 20),
        const Text('Sound Effect Volume', style: TextStyle(fontSize: 16)),
        ListenableBuilder(
            listenable: widget.controller,
            builder: (BuildContext context, Widget? child) {
              return Slider(
                  value: widget.controller.soundVolume,
                  onChanged: (value) {
                    widget.controller.updateSoundVolume(value);
                    widget.player.updateOutputVolume(volume: value);
                  },
                  min: -20,
                  max: 20,
                  label:
                      '${widget.controller.soundVolume.toStringAsFixed(2)} db');
            }),
        const SizedBox(height: 5),
        Row(
          mainAxisAlignment: MainAxisAlignment.spaceBetween,
          mainAxisSize: MainAxisSize.max,
          children: [
            const Text('Enable Efficiency Mode',
                style: TextStyle(fontSize: 18)),
            ListenableBuilder(
                listenable: widget.controller,
                builder: (BuildContext context, Widget? child) {
                  return CustomSwitch(
                      value: widget.controller.efficiencyMode,
                      onChanged: (enabled) {
                        widget.controller.updateEfficiencyMode(enabled);
                        widget.telepathy.setEfficiencyMode(enabled: enabled);
                      });
                }),
          ],
        ),
        const Divider(),
        const Text(
          'Screenshare Options',
          style: TextStyle(fontSize: 20),
        ),
        const SizedBox(height: 17),
        Wrap(
          spacing: 20,
          runSpacing: 20,
          children: [
            DropDown(
              label: 'Encoder',
              items: encoders.map((d) => (d, d)).toList(),
              initialSelection:
                  _recordingConfig?.encoder() ?? encoders.firstOrNull,
              onSelected: (String? value) {
                if (value == null) {
                  return;
                } else if (_temporaryConfig == null) {
                  initTemporaryConfig(value, null, null, null);
                } else {
                  setState(() {
                    _temporaryConfig!.encoder = value;
                  });
                }
              },
              width: width,
            ),
            DropDown(
              label: 'Capture Device',
              items: devices.map((d) => (d, d)).toList(),
              initialSelection:
                  _recordingConfig?.device() ?? devices.firstOrNull,
              onSelected: (String? value) {
                if (value == null) {
                  return;
                } else if (_temporaryConfig == null) {
                  initTemporaryConfig(null, value, null, null);
                } else {
                  setState(() {
                    _temporaryConfig!.device = value;
                  });
                }
              },
              width: width,
            )
          ],
        ),
        const SizedBox(height: 20),
        Button(
            text: _loading ? 'Verifying' : 'Save',
            disabled: _temporaryConfig == null || _loading,
            onPressed: () async {
              try {
                if (_loading) return;

                setState(() {
                  _loading = true;
                });

                await widget.controller.screenshareConfig.updateRecordingConfig(
                    encoder: _temporaryConfig!.encoder,
                    device: _temporaryConfig!.device,
                    bitrate: _temporaryConfig!.bitrate,
                    framerate: _temporaryConfig!.framerate,
                    height: _temporaryConfig!.height);

                widget.controller.saveScreenshareConfig();

                setState(() {
                  _temporaryConfig = null;
                  _loading = false;
                });
              } on DartError catch (e) {
                setState(() {
                  _loading = false;
                });
                if (!context.mounted) return;
                showErrorDialog(
                    context, 'Error in Encoder Selection', e.message);
              }
            }),
      ],
    );
  }
}

class ProfileSettings extends StatefulWidget {
  final SettingsController controller;
  final Telepathy telepathy;
  final StateController stateController;

  const ProfileSettings(
      {super.key,
      required this.controller,
      required this.telepathy,
      required this.stateController});

  @override
  ProfileSettingsState createState() => ProfileSettingsState();
}

class ProfileSettingsState extends State<ProfileSettings> {
  final TextEditingController _profileNameInput = TextEditingController();

  @override
  Widget build(BuildContext context) {
    return Container(
      decoration: BoxDecoration(
        color: Theme.of(context).colorScheme.surface,
        borderRadius: BorderRadius.circular(5),
      ),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.end,
        mainAxisSize: MainAxisSize.min,
        children: [
          ListenableBuilder(
              listenable: widget.controller,
              builder: (BuildContext context, Widget? child) {
                bool even = widget.controller.profiles.length % 2 == 0;

                Color colorPicker(int index) {
                  if (even ? index % 2 == 0 : index % 2 != 0) {
                    return Colors.transparent;
                  } else {
                    return Theme.of(context).colorScheme.secondaryContainer;
                  }
                }

                return ListView.builder(
                    shrinkWrap: true,
                    itemCount: widget.controller.profiles.length,
                    itemBuilder: (BuildContext context, int index) {
                      Profile profile =
                          widget.controller.profiles.values.elementAt(index);

                      return Container(
                        decoration: BoxDecoration(
                          color: colorPicker(index),
                          borderRadius: index == 0
                              ? const BorderRadius.only(
                                  topLeft: Radius.circular(5),
                                  topRight: Radius.circular(5))
                              : null,
                        ),
                        padding: const EdgeInsets.only(
                            top: 5, bottom: 5, left: 20, right: 10),
                        child: Row(
                          children: [
                            Text(
                              profile.nickname,
                              style: const TextStyle(fontSize: 18),
                            ),
                            const Spacer(),
                            ListenableBuilder(
                                listenable: widget.stateController,
                                builder: (BuildContext context, Widget? child) {
                                  return Button(
                                    text: (widget.controller.activeProfile ==
                                            profile.id)
                                        ? 'Active'
                                        : 'Set Active',
                                    width: 65,
                                    height: 25,
                                    disabled:
                                        widget.stateController.isCallActive ||
                                            widget.controller.activeProfile ==
                                                profile.id,
                                    onPressed: () {
                                      widget.controller
                                          .setActiveProfile(profile.id);
                                      widget.telepathy
                                          .setIdentity(key: profile.keypair);
                                      widget.telepathy.restartManager();
                                    },
                                    noSplash: true,
                                    disabledColor: widget
                                                    .controller.activeProfile ==
                                                profile.id &&
                                            widget.stateController.isCallActive
                                        ? Theme.of(context)
                                            .colorScheme
                                            .tertiaryContainer
                                        : null,
                                  );
                                }),
                            const SizedBox(width: 10),
                            IconButton(
                                tooltip: 'Copy Peer ID',
                                onPressed: () {
                                  Clipboard.setData(
                                      ClipboardData(text: profile.peerId));
                                },
                                icon: SvgPicture.asset(
                                  'assets/icons/Copy.svg',
                                  semanticsLabel: 'Copy Peer ID',
                                  width: 26,
                                )),
                            IconButton(
                              tooltip: 'Delete Profile',
                              onPressed: () {
                                showDialog(
                                    context: context,
                                    builder: (BuildContext context) {
                                      return AlertDialog(
                                        title: const Text('Delete Profile'),
                                        content: const Text(
                                            'Are you sure you want to delete this profile?'),
                                        actions: [
                                          Button(
                                            text: 'Cancel',
                                            onPressed: () {
                                              Navigator.of(context).pop();
                                            },
                                          ),
                                          Button(
                                            text: 'Delete',
                                            onPressed: () {
                                              widget.controller
                                                  .removeProfile(profile.id);
                                              Navigator.of(context).pop();
                                            },
                                          )
                                        ],
                                      );
                                    });
                              },
                              icon: SvgPicture.asset(
                                'assets/icons/Trash.svg',
                                semanticsLabel: 'Delete Profile',
                                width: 26,
                              ),
                            ),
                          ],
                        ),
                      );
                    });
              }),
          Padding(
            padding: const EdgeInsets.symmetric(vertical: 5, horizontal: 20),
            child: IconButton(
              onPressed: () {
                showDialog(
                    context: context,
                    builder: (BuildContext context) {
                      return SimpleDialog(
                        title: const Text('Create Profile'),
                        contentPadding: const EdgeInsets.only(
                            bottom: 25, left: 25, right: 25),
                        titlePadding: const EdgeInsets.only(
                            top: 25, left: 25, right: 25, bottom: 15),
                        children: [
                          TextField(
                            decoration: const InputDecoration(
                              labelText: 'Name',
                            ),
                            controller: _profileNameInput,
                          ),
                          const SizedBox(height: 20),
                          Button(
                            text: 'Create',
                            onPressed: () {
                              widget.controller
                                  .createProfile(_profileNameInput.text);
                              _profileNameInput.clear();
                              Navigator.of(context).pop();
                            },
                          )
                        ],
                      );
                    });
              },
              visualDensity: VisualDensity.comfortable,
              icon: SvgPicture.asset(
                'assets/icons/Plus.svg',
                semanticsLabel: 'Create Profile',
                width: 38,
              ),
              tooltip: 'Create Profile',
            ),
          ),
        ],
      ),
    );
  }
}

class NetworkSettings extends StatefulWidget {
  final SettingsController controller;
  final Telepathy telepathy;
  final StateController stateController;
  final BoxConstraints constraints;

  const NetworkSettings(
      {super.key,
      required this.controller,
      required this.telepathy,
      required this.stateController,
      required this.constraints});

  @override
  NetworkSettingsState createState() => NetworkSettingsState();
}

class NetworkSettingsState extends State<NetworkSettings> {
  late String _relayAddress;
  late String _relayPeerId;
  bool unsavedChanges = false;

  final TextEditingController _relayAddressInput = TextEditingController();
  String? _relayAddressError;

  final TextEditingController _relayPeerIdInput = TextEditingController();
  String? _relayPeerIdError;

  @override
  void initState() {
    super.initState();
    _initialize();
  }

  Future<void> _initialize() async {
    _relayAddress = await widget.controller.networkConfig.getRelayAddress();
    _relayPeerId = await widget.controller.networkConfig.getRelayId();

    _relayAddressInput.text = _relayAddress;
    _relayPeerIdInput.text = _relayPeerId;
  }

  @override
  Widget build(BuildContext context) {
    double width = widget.constraints.maxWidth < 650
        ? widget.constraints.maxWidth
        : (widget.constraints.maxWidth - 20) / 2;

    return Column(
      mainAxisSize: MainAxisSize.min,
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        Center(
          child: Wrap(
            spacing: 20,
            runSpacing: 20,
            children: [
              SizedBox(
                  width: width,
                  child: TextInput(
                    labelText: 'Relay Address',
                    controller: _relayAddressInput,
                    onChanged: (String value) {
                      if (value != _relayAddress) {
                        setState(() {
                          unsavedChanges = true;
                        });
                      }
                    },
                    error: _relayAddressError == null
                        ? null
                        : Text(_relayAddressError!,
                            style: const TextStyle(color: Colors.red)),
                  )),
              SizedBox(
                  width: width,
                  child: TextInput(
                    labelText: 'Relay Peer ID',
                    controller: _relayPeerIdInput,
                    onChanged: (String value) {
                      if (value != _relayPeerId) {
                        setState(() {
                          unsavedChanges = true;
                        });
                      }
                    },
                    error: _relayPeerIdError == null
                        ? null
                        : Text(_relayPeerIdError!,
                            style: const TextStyle(color: Colors.red)),
                  )),
            ],
          ),
        ),
        if (unsavedChanges) const SizedBox(height: 20),
        if (unsavedChanges)
          Button(
            text: 'Save Changes',
            onPressed: saveChanges,
            width: 100,
          ),
      ],
    );
  }

  Future<void> saveChanges() async {
    String relayAddress = _relayAddressInput.text;
    String relayId = _relayPeerIdInput.text;

    bool changed = false;

    try {
      // this will raise an error if the relay ID isn't formatted right
      await widget.controller.networkConfig.setRelayId(relayId: relayId);
      _relayPeerId = relayId;
      changed = true;
      setState(() {
        _relayPeerIdError = null;
      });
    } on DartError catch (error) {
      setState(() {
        _relayPeerIdError = error.message;
      });
    }

    try {
      // this will raise an error if the relay address isn't a valid socket address
      await widget.controller.networkConfig
          .setRelayAddress(relayAddress: relayAddress);
      _relayAddress = relayAddress;
      changed = true;
      setState(() {
        _relayAddressError = null;
      });
    } on DartError catch (error) {
      setState(() {
        _relayAddressError = error.message;
      });
    }

    unsavedChanges = _relayAddressError != null || _relayPeerIdError != null;

    if (changed) {
      widget.controller.saveNetworkConfig();
      widget.telepathy.restartManager();
    }
  }
}

class InterfaceSettings extends StatefulWidget {
  final InterfaceController controller;
  final BoxConstraints constraints;

  const InterfaceSettings(
      {super.key, required this.controller, required this.constraints});

  @override
  InterfaceSettingsState createState() => InterfaceSettingsState();
}

class InterfaceSettingsState extends State<InterfaceSettings> {
  final TextEditingController _primaryColorInput = TextEditingController();
  String? _primaryColorError;

  @override
  void initState() {
    super.initState();
    _primaryColorInput.text =
        "#${widget.controller.primaryColor.toRadixString(16)}";
  }

  @override
  Widget build(BuildContext context) {
    double width = widget.constraints.maxWidth < 650
        ? widget.constraints.maxWidth
        : (widget.constraints.maxWidth - 20) / 2;

    return Column(
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        Center(
          child: Wrap(
            spacing: 20,
            runSpacing: 20,
            children: [
              SizedBox(
                  width: width,
                  child: TextInput(
                    labelText: 'Primary Color',
                    controller: _primaryColorInput,
                    onChanged: (String value) {
                      int? color =
                          int.tryParse(value.replaceAll('#', ''), radix: 16);

                      if (color == null) {
                        _primaryColorError = 'Invalid hex color';
                      } else {
                        _primaryColorError = null;
                        widget.controller.setPrimaryColor(color);
                      }
                    },
                    error: _primaryColorError == null
                        ? null
                        : Text(_primaryColorError!,
                            style: const TextStyle(color: Colors.red)),
                  )),
              Button(
                text: 'Revert primary color to default',
                onPressed: () {
                  widget.controller.setPrimaryColor(0xff5538e5);
                  _primaryColorInput.text = '#ff5538e5';
                },
                width: 200,
                height: 25,
              ),
            ],
          ),
        ),
      ],
    );
  }
}

class OverlaySettings extends StatefulWidget {
  final Overlay overlay;
  final SettingsController controller;
  final StateController stateController;

  const OverlaySettings(
      {super.key,
      required this.overlay,
      required this.controller,
      required this.stateController});

  @override
  OverlaySettingsState createState() => OverlaySettingsState();
}

class OverlaySettingsState extends State<OverlaySettings> {
  bool overlayVisible = false;

  @override
  void dispose() {
    if (!widget.stateController.isCallActive) {
      widget.overlay.hide_();
    }

    widget.controller.saveOverlayConfig();

    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    (int, int) size = widget.overlay.screenResolution();

    return Column(
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        Row(
          children: [
            Button(
              text: overlayVisible ? "Hide overlay" : "Show overlay",
              onPressed: () {
                if (widget.stateController.isCallActive ||
                    !widget.controller.overlayConfig.enabled) {
                  return;
                } else if (overlayVisible) {
                  widget.overlay.hide_();
                } else {
                  widget.overlay.show_();
                }

                setState(() {
                  overlayVisible = !overlayVisible;
                });
              },
              disabled: widget.stateController.isCallActive ||
                  !widget.controller.overlayConfig.enabled,
              width: 90,
              height: 25,
            ),
            const SizedBox(width: 20),
            Button(
              text: widget.controller.overlayConfig.enabled
                  ? "Disable overlay"
                  : "Enable overlay",
              onPressed: () async {
                if (widget.controller.overlayConfig.enabled) {
                  await widget.overlay.disable();
                  widget.controller.overlayConfig.enabled = false;

                  // the overlay is never visible when it is disabled
                  setState(() {
                    overlayVisible = false;
                  });
                } else {
                  await widget.overlay.enable();
                  widget.controller.overlayConfig.enabled = true;

                  if (widget.stateController.isCallActive) {
                    // if the call is active, the overlay should be shown
                    widget.overlay.show_();

                    setState(() {
                      overlayVisible = true;
                    });
                  } else {
                    // if the call is not active, the overlay should be hidden
                    setState(() {
                      overlayVisible = false;
                    });
                  }
                }

                // save the config
                widget.controller.saveOverlayConfig();
              },
              width: 110,
              height: 25,
            ),
          ],
        ),
        const SizedBox(height: 20),
        const Text('Font Size', style: TextStyle(fontSize: 18)),
        Slider(
            value: widget.controller.overlayConfig.fontHeight.toDouble(),
            onChanged: (value) {
              widget.overlay.setFontHeight(height: value.round());
              widget.controller.overlayConfig.fontHeight = value.round();
              widget.controller.saveOverlayConfig();
              setState(() {});
            },
            min: 0,
            max: 70),
        const SizedBox(height: 15),
        Row(
          children: [
            Column(
              crossAxisAlignment: CrossAxisAlignment.start,
              children: [
                const Text('Background Color', style: TextStyle(fontSize: 18)),
                const SizedBox(height: 10),
                Button(
                    text: 'Change',
                    onPressed: () {
                      colorPicker(context, (Color color) {
                        widget.overlay.setBackgroundColor(
                            backgroundColor: color.toARGB32());
                        widget.controller.overlayConfig.backgroundColor = color;
                        widget.controller.saveOverlayConfig();
                        setState(() {});
                      }, widget.controller.overlayConfig.backgroundColor);
                    }),
              ],
            ),
            const SizedBox(width: 40),
            Column(
              crossAxisAlignment: CrossAxisAlignment.start,
              children: [
                const Text('Primary Font Color',
                    style: TextStyle(fontSize: 18)),
                const SizedBox(height: 10),
                Button(
                    text: 'Change',
                    onPressed: () {
                      colorPicker(context, (Color color) {
                        widget.overlay
                            .setFontColor(fontColor: color.toARGB32());
                        widget.controller.overlayConfig.fontColor = color;
                        widget.controller.saveOverlayConfig();
                        setState(() {});
                      }, widget.controller.overlayConfig.fontColor);
                    }),
              ],
            )
          ],
        ),
        const SizedBox(height: 35),
        if (size.$1 > 0 && size.$2 > 0)
          OverlayPositionWidget(
            overlay: widget.overlay,
            controller: widget.controller,
            realMaxX: size.$1.toDouble(),
            realMaxY: size.$2.toDouble(),
          ),
      ],
    );
  }
}

class OverlayPositionWidget extends StatefulWidget {
  final Overlay overlay;
  final SettingsController controller;

  final double realMaxX;
  final double realMaxY;

  const OverlayPositionWidget(
      {super.key,
      required this.overlay,
      required this.realMaxX,
      required this.realMaxY,
      required this.controller});

  @override
  OverlayPositionWidgetState createState() => OverlayPositionWidgetState();
}

class OverlayPositionWidgetState extends State<OverlayPositionWidget> {
  late double _maxX;
  late double _maxY;
  late double _x;
  late double _y;
  late double _width;
  late double _height;

  bool _isDragging = false;
  bool _isResizing = false;

  @override
  void initState() {
    super.initState();

    _maxX = 650.0;
    _updatePositions();
  }

  void _updatePositions() {
    _maxY = _maxX / (widget.realMaxX / widget.realMaxY);

    _x = widget.controller.overlayConfig.x / widget.realMaxX * _maxX;
    _y = widget.controller.overlayConfig.y / widget.realMaxY * _maxY;
    _width = widget.controller.overlayConfig.width / widget.realMaxX * _maxX;
    _height = widget.controller.overlayConfig.height / widget.realMaxY * _maxY;
  }

  void _updateOverlay() {
    double realX = _x / _maxX * widget.realMaxX;
    double realY = _y / _maxY * widget.realMaxY;
    double realWidth = _width / _maxX * widget.realMaxX;
    double realHeight = _height / _maxY * widget.realMaxY;

    widget.overlay.moveOverlay(
      x: realX.round(),
      y: realY.round(),
      width: realWidth.round(),
      height: realHeight.round(),
    );

    widget.controller.overlayConfig.x = realX;
    widget.controller.overlayConfig.y = realY;
    widget.controller.overlayConfig.width = realWidth;
    widget.controller.overlayConfig.height = realHeight;
  }

  void _onDragUpdate(DragUpdateDetails details) {
    if (_isDragging) {
      setState(() {
        _x += details.delta.dx;
        _y += details.delta.dy;

        if (_x < 0) {
          _x = 0;
        } else if (_x + _width > _maxX) {
          _x = _maxX - _width;
        }

        if (_y < 0) {
          _y = 0;
        } else if (_y + _height > _maxY) {
          _y = _maxY - _height;
        }

        _updateOverlay();
      });
    }
  }

  void _onResizeUpdate(DragUpdateDetails details) {
    if (_isResizing) {
      setState(() {
        _width += details.delta.dx;
        _height += details.delta.dy;

        if (_width + _x > _maxX) {
          _width = _maxX - _x;
        } else if (_width < 10) {
          _width = 10;
        }

        if (_height + _y > _maxY) {
          _height = _maxY - _y;
        } else if (_height < 10) {
          _height = 10;
        }

        _updateOverlay();
      });
    }
  }

  void _startDragging() {
    setState(() {
      _isDragging = true;
    });
  }

  void _startResizing() {
    setState(() {
      _isResizing = true;
    });
  }

  void _stopDragging() {
    setState(() {
      _isDragging = false;
    });
  }

  void _stopResizing() {
    setState(() {
      _isResizing = false;
    });
  }

  @override
  Widget build(BuildContext context) {
    return LayoutBuilder(
      builder: (context, constraints) {
        _maxX = constraints.maxWidth;
        _updatePositions();

        return Container(
          decoration: BoxDecoration(
            border: Border.all(color: Colors.black, width: 2),
            color: Theme.of(context).colorScheme.surface,
          ),
          height: _maxY,
          child: Stack(
            children: [
              Positioned(
                left: _x,
                top: _y,
                child: GestureDetector(
                  onPanUpdate: _onDragUpdate,
                  onPanStart: (_) => _startDragging(),
                  onPanEnd: (_) => _stopDragging(),
                  child: Container(
                      decoration: BoxDecoration(
                        color: widget.controller.overlayConfig.backgroundColor,
                        border: Border.all(
                            color: Theme.of(context).colorScheme.secondary,
                            width: 2),
                      ),
                      child: MouseRegion(
                        cursor: SystemMouseCursors.move,
                        child: SizedBox(
                          width: _width,
                          height: _height,
                        ),
                      )),
                ),
              ),
              Positioned(
                left: _x + _width - 10,
                top: _y + _height - 10,
                child: GestureDetector(
                  onPanUpdate: _onResizeUpdate,
                  onPanStart: (_) => _startResizing(),
                  onPanEnd: (_) => _stopResizing(),
                  child: const MouseRegion(
                    cursor: SystemMouseCursors.resizeDownRight,
                    child: SizedBox(
                      width: 20,
                      height: 20,
                    ),
                  ),
                ),
              ),
            ],
          ),
        );
      },
    );
  }
}

class DropDown<T> extends StatelessWidget {
  final String? label;
  final List<(String, String)> items;
  final String? initialSelection;
  final void Function(String?) onSelected;
  final double? width;
  final bool enabled;

  const DropDown(
      {super.key,
      this.label,
      required this.items,
      required this.initialSelection,
      required this.onSelected,
      this.width,
      this.enabled = true});

  @override
  Widget build(BuildContext context) {
    return DropdownMenu<String>(
      width: width,
      label: label == null ? null : Text(label!),
      enabled: enabled,
      dropdownMenuEntries: items.map((item) {
        return DropdownMenuEntry(
          value: item.$1,
          label: item.$2,
        );
      }).toList(),
      onSelected: onSelected,
      initialSelection: initialSelection,
      trailingIcon: SvgPicture.asset(
        'assets/icons/DropdownDown.svg',
        semanticsLabel: 'Open Dropdown',
        width: 20,
      ),
      selectedTrailingIcon: SvgPicture.asset(
        'assets/icons/DropdownUp.svg',
        semanticsLabel: 'Close Dropdown',
        width: 20,
      ),
    );
  }
}

class AudioDevices extends ChangeNotifier {
  final Telepathy telepathy;
  Timer? periodicTimer;

  late List<String> _inputDevices = [];
  late List<String> _outputDevices = [];

  final ListEquality<String> _listEquality = const ListEquality<String>();

  List<String> get inputDevices => ['Default', ..._inputDevices];

  List<String> get outputDevices => ['Default', ..._outputDevices];

  AudioDevices({required this.telepathy}) {
    DebugConsole.debug('AudioDevices created');
    updateDevices();
  }

  @override
  void dispose() {
    periodicTimer?.cancel();
    super.dispose();
  }

  void updateDevices() async {
    var (inputDevices, outputDevices) = await telepathy.listDevices();

    bool notify = false;

    if (!_listEquality.equals(_inputDevices, inputDevices)) {
      _inputDevices = inputDevices;
      notify = true;
    }

    if (!_listEquality.equals(_outputDevices, outputDevices)) {
      _outputDevices = outputDevices;
      notify = true;
    }

    if (notify) {
      notifyListeners();
    }
  }

  void startUpdates() {
    periodicTimer = Timer.periodic(const Duration(milliseconds: 500), (timer) {
      updateDevices();
    });
  }

  void pauseUpdates() {
    periodicTimer?.cancel();
  }
}

class TemporaryConfig {
  String encoder;
  String device;
  int bitrate;
  int framerate;
  int? height;

  TemporaryConfig(
      {required this.encoder,
      required this.device,
      required this.bitrate,
      required this.framerate,
      this.height});
}

void colorPicker(BuildContext context, void Function(Color) changeColor,
    Color currentColor) {
  showDialog(
    context: context,
    builder: (BuildContext context) {
      return AlertDialog(
        title: const Text('Pick a color'),
        content: SingleChildScrollView(
          child: ColorPicker(
            pickerColor: currentColor,
            onColorChanged: changeColor,
          ),
        ),
        actions: <Widget>[
          Button(
            text: 'Close',
            onPressed: () {
              Navigator.of(context).pop();
            },
          ),
        ],
      );
    },
  );
}

Future<bool> unsavedConfirmation(BuildContext context) async {
  bool? result = await showDialog<bool>(
    context: context,
    builder: (BuildContext context) {
      return AlertDialog(
        title: const Text('Unsaved Changes'),
        content: const Text(
            'You have unsaved changes. Are you sure you want to leave?'),
        actions: [
          Button(
            text: 'Cancel',
            onPressed: () {
              Navigator.of(context).pop(false);
            },
          ),
          Button(
            text: 'Leave',
            onPressed: () {
              Navigator.of(context).pop(true);
            },
          )
        ],
      );
    },
  );

  return result ?? false;
}
