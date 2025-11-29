import 'dart:async';
import 'dart:core';
import 'dart:io';
import 'package:flutter/foundation.dart' show kIsWeb;
import 'package:telepathy/settings/controller.dart';
import 'package:telepathy/settings/sections/audio_video.dart';
import 'package:telepathy/settings/sections/interface.dart';
import 'package:telepathy/settings/sections/networking.dart';
import 'package:telepathy/settings/sections/overlay.dart';
import 'package:telepathy/settings/sections/profiles.dart';
import 'package:telepathy/src/rust/audio/player.dart';
import 'package:collection/collection.dart';
import 'package:flutter/material.dart' hide Overlay;
import 'package:flutter_svg/flutter_svg.dart';
import 'package:telepathy/console.dart';
import 'package:telepathy/main.dart';
import 'package:telepathy/src/rust/telepathy.dart';
import 'package:telepathy/src/rust/overlay/overlay.dart';
import 'package:telepathy/settings/logs.dart';
import 'package:telepathy/settings/header.dart';
import 'package:telepathy/settings/menu.dart';

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
                              } else if (_section ==
                                  SettingsSection.networking) {
                                return NetworkSettings(
                                    key: _key,
                                    controller: widget.controller,
                                    telepathy: widget.telepathy,
                                    stateController: widget.stateController,
                                    constraints: constraints);
                              } else if (_section ==
                                  SettingsSection.interface) {
                                return InterfaceSettings(
                                    controller: widget.interfaceController,
                                    constraints: constraints);
                              } else if (_section == SettingsSection.logs) {
                                return LogsSettings(
                                    searchController: _searchController);
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
                    onHoverChanged: (idx, isHovered) =>
                        hoverHandler(idx, isHovered),
                    showOverlayItem: !kIsWeb && Platform.isWindows,
                  ),
                ),
              ),
            SettingsHeader(
              isNarrow: constraints.maxWidth < 600,
              showMenu: showMenu ?? true,
              onBack: () async {
                if (_section == SettingsSection.networking &&
                    (_key.currentState?.unsavedChanges ?? false)) {
                  bool leave = await unsavedConfirmation(context);
                  if (!leave) return;
                }

                if (context.mounted) {
                  Navigator.of(context).pop();
                }
              },
              onToggleMenu: () {
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
        ));
  }

  Future<void> tapHandler(SettingsSection target) async {
    if (_section == SettingsSection.networking &&
        (_key.currentState?.unsavedChanges ?? false)) {
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
