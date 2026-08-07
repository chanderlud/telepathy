import 'dart:core';

import 'package:flutter/foundation.dart' show kIsWeb;
import 'package:flutter/material.dart' hide Overlay;
import 'package:telepathy/core/utils/index.dart';
import 'package:telepathy/screens/settings/header.dart';
import 'package:telepathy/screens/settings/logs.dart';
import 'package:telepathy/screens/settings/menu.dart';
import 'package:telepathy/screens/settings/sections/audio_video.dart';
import 'package:telepathy/screens/settings/sections/interface.dart';
import 'package:telepathy/screens/settings/sections/networking.dart';
import 'package:telepathy/screens/settings/sections/overlay.dart';
import 'package:telepathy/screens/settings/sections/profiles.dart';

enum SettingsSection {
  audioVideo,
  profiles,
  networking,
  interface,
  logs,
  overlay,
}

class SettingsPage extends StatefulWidget {
  final BoxConstraints constraints;

  const SettingsPage({super.key, required this.constraints});

  @override
  SettingsPageState createState() => SettingsPageState();
}

class SettingsPageState extends State<SettingsPage>
    with SingleTickerProviderStateMixin {
  SettingsSection _section = SettingsSection.audioVideo;
  bool? showMenu;

  final TextEditingController _searchController = TextEditingController();
  final GlobalKey<NetworkSettingsState> _key =
      GlobalKey<NetworkSettingsState>();

  late AnimationController _animationController;
  late Animation<Offset> _menuSlideAnimation;
  final FocusNode _menuFocusNode = FocusNode();
  final Object _menuTapRegionGroup = Object();

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
    _menuFocusNode.dispose();
    _animationController.dispose();
    super.dispose();
  }

  double _contentTopPadding(bool isNarrow, SettingsSection section) {
    if (isNarrow) {
      return section == SettingsSection.audioVideo ? 55 : 70;
    } else {
      return section == SettingsSection.audioVideo ? 10 : 30;
    }
  }

  void _setMenuVisibility(bool visible) {
    if (showMenu == visible) return;

    setState(() {
      if (visible) {
        _animationController.reverse();
      } else {
        _animationController.forward();
      }
      showMenu = visible;
    });

    if (visible) {
      WidgetsBinding.instance.addPostFrameCallback((_) {
        if (mounted && showMenu == true && widget.constraints.maxWidth < 600) {
          _menuFocusNode.requestFocus();
        }
      });
    }
  }

  void _dismissNarrowMenu() {
    if (widget.constraints.maxWidth >= 600 || showMenu != true) return;
    _setMenuVisibility(false);
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
                              top: _contentTopPadding(
                                  constraints.maxWidth < 600, _section)),
                          child: SizedBox(
                            width: width,
                            child: LayoutBuilder(builder: (BuildContext context,
                                BoxConstraints constraints) {
                              return switch (_section) {
                                SettingsSection.audioVideo => AVSettings(
                                    constraints: constraints,
                                  ),
                                SettingsSection.profiles =>
                                  const ProfileSettings(),
                                SettingsSection.networking => NetworkSettings(
                                    key: _key, constraints: constraints),
                                SettingsSection.interface =>
                                  InterfaceSettings(constraints: constraints),
                                SettingsSection.logs => LogsSettings(
                                    searchController: _searchController),
                                SettingsSection.overlay =>
                                  const OverlaySettings(),
                              };
                            }),
                          ),
                        )
                      ],
                    ),
                  )),
            ),
            Positioned.fill(
              child: Focus(
                focusNode: _menuFocusNode,
                onFocusChange: (hasFocus) {
                  if (!hasFocus) _dismissNarrowMenu();
                },
                child: Stack(
                  children: [
                    if (constraints.maxWidth > 600 || (showMenu ?? true))
                      TapRegion(
                        groupId: _menuTapRegionGroup,
                        onTapOutside: (_) => _dismissNarrowMenu(),
                        child: SlideTransition(
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
                              onSectionSelected: (section) =>
                                  tapHandler(section),
                              showOverlayItem: !kIsWeb && Platform.isWindows,
                            ),
                          ),
                        ),
                      ),
                    TapRegion(
                      groupId: _menuTapRegionGroup,
                      child: SettingsHeader(
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
                        onToggleMenu: () =>
                            _setMenuVisibility(!(showMenu ?? true)),
                      ),
                    ),
                  ],
                ),
              ),
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
}
