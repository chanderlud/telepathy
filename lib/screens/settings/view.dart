import 'dart:core';
import 'dart:io';

import 'package:flutter/foundation.dart' show kIsWeb;
import 'package:flutter/material.dart' hide Overlay;
import 'package:telepathy/controllers/index.dart';
import 'package:telepathy/core/utils/index.dart';
import 'package:telepathy/screens/settings/header.dart';
import 'package:telepathy/screens/settings/logs.dart';
import 'package:telepathy/screens/settings/menu.dart';
import 'package:telepathy/screens/settings/sections/audio_video.dart';
import 'package:telepathy/screens/settings/sections/interface.dart';
import 'package:telepathy/screens/settings/sections/networking.dart';
import 'package:telepathy/screens/settings/sections/overlay.dart';
import 'package:telepathy/screens/settings/sections/profiles.dart';
import 'package:telepathy/src/rust/audio/player.dart';
import 'package:telepathy/src/rust/overlay/overlay.dart';
import 'package:telepathy/src/rust/telepathy.dart';

enum SettingsSection {
  audioVideo,
  profiles,
  networking,
  interface,
  logs,
  overlay,
}

class SettingsPage extends StatefulWidget {
  final ProfilesController profilesController;
  final AudioSettingsController audioSettingsController;
  final NetworkSettingsController networkSettingsController;
  final PreferencesController preferencesController;
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
      required this.profilesController,
      required this.audioSettingsController,
      required this.networkSettingsController,
      required this.preferencesController,
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
  SettingsSection? hovered;
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
                                  audioSettingsController:
                                      widget.audioSettingsController,
                                  preferencesController:
                                      widget.preferencesController,
                                  networkSettingsController:
                                      widget.networkSettingsController,
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
                                    profilesController:
                                        widget.profilesController,
                                    telepathy: widget.telepathy,
                                    stateController: widget.stateController);
                              } else if (_section ==
                                  SettingsSection.networking) {
                                return NetworkSettings(
                                    key: _key,
                                    networkSettingsController:
                                        widget.networkSettingsController,
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
                                    networkSettingsController:
                                        widget.networkSettingsController,
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
                    hovered: hovered,
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

  void hoverHandler(SettingsSection target, bool hovered) {
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
