import 'package:flutter/foundation.dart' show kIsWeb;
import 'package:flutter/material.dart' hide Overlay;
import 'package:telepathy/core/theme/app_theme.dart';
import 'package:telepathy/controllers/index.dart';
import 'package:telepathy/screens/home/home_page.dart';

import 'package:telepathy/src/rust/audio/player.dart';
import 'package:telepathy/src/rust/overlay/overlay.dart';
import 'package:telepathy/src/rust/telepathy.dart';
import 'package:window_manager/window_manager.dart';

final GlobalKey<NavigatorState> navigatorKey = GlobalKey<NavigatorState>();

/// The main app
class TelepathyApp extends StatefulWidget {
  final Telepathy telepathy;
  final ProfilesController profilesController;
  final AudioSettingsController audioSettingsController;
  final NetworkSettingsController networkSettingsController;
  final PreferencesController preferencesController;
  final InterfaceController interfaceController;
  final StateController callStateController;
  final StatisticsController statisticsController;
  final SoundPlayer player;
  final ChatStateController chatStateController;
  final Overlay overlay;
  final AudioDevices audioDevices;

  const TelepathyApp(
      {super.key,
      required this.telepathy,
      required this.profilesController,
      required this.audioSettingsController,
      required this.networkSettingsController,
      required this.preferencesController,
      required this.callStateController,
      required this.player,
      required this.chatStateController,
      required this.statisticsController,
      required this.overlay,
      required this.audioDevices,
      required this.interfaceController});

  @override
  State<StatefulWidget> createState() => _TelepathyAppState();
}

class _TelepathyAppState extends State<TelepathyApp> with WindowListener {
  bool _isClosing = false;

  @override
  void initState() {
    super.initState();
    windowManager.addListener(this);
    _initWindow();
  }

  Future<void> _initWindow() async {
    if (!kIsWeb) {
      await windowManager.setPreventClose(true);
    }
  }

  @override
  void dispose() {
    windowManager.removeListener(this);
    super.dispose();
  }

  @override
  void onWindowClose() async {
    // second click (or more): force close, ignore whatever shutdown is doing
    if (!_isClosing) {
      _isClosing = true;
      await widget.telepathy.shutdown();
    }

    await windowManager.setPreventClose(false);
    await windowManager.close();
  }

  @override
  Widget build(BuildContext context) {
    return ListenableBuilder(
        listenable: widget.interfaceController,
        builder: (BuildContext context, Widget? child) {
          return MaterialApp(
            navigatorKey: navigatorKey,
            theme: AppTheme.dark(
              context,
              primaryColor: widget.interfaceController.primaryColor,
              secondaryColor: widget.interfaceController.secondaryColor,
            ),
            home: HomePage(
              telepathy: widget.telepathy,
              profilesController: widget.profilesController,
              audioSettingsController: widget.audioSettingsController,
              networkSettingsController: widget.networkSettingsController,
              preferencesController: widget.preferencesController,
              interfaceController: widget.interfaceController,
              stateController: widget.callStateController,
              player: widget.player,
              chatStateController: widget.chatStateController,
              statisticsController: widget.statisticsController,
              overlay: widget.overlay,
              audioDevices: widget.audioDevices,
            ),
          );
        });
  }
}
