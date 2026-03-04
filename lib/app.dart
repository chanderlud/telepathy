import 'package:flutter/foundation.dart' show kIsWeb;
import 'package:flutter/material.dart' hide Overlay;
import 'package:provider/provider.dart';
import 'package:telepathy/core/theme/app_theme.dart';
import 'package:telepathy/controllers/index.dart';
import 'package:telepathy/screens/home/home_page.dart';

import 'package:telepathy/src/rust/telepathy.dart';
import 'package:window_manager/window_manager.dart';

final GlobalKey<NavigatorState> navigatorKey = GlobalKey<NavigatorState>();

/// The main app
class TelepathyApp extends StatefulWidget {
  const TelepathyApp({super.key});

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
      await context.read<Telepathy>().shutdown();
    }

    await windowManager.setPreventClose(false);
    await windowManager.close();
  }

  @override
  void onWindowMinimize() {
    context.read<Telepathy>().pauseStatistics();
  }

  @override
  void onWindowMaximize() {
    context.read<Telepathy>().resumeStatistics();
  }

  @override
  void onWindowRestore() {
    context.read<Telepathy>().resumeStatistics();
  }

  @override
  Widget build(BuildContext context) {
    final interfaceController = context.watch<InterfaceController>();

    return MaterialApp(
      title: 'Telepathy',
      navigatorKey: navigatorKey,
      theme: AppTheme.dark(
        context,
        primaryColor: interfaceController.primaryColor,
        secondaryColor: interfaceController.secondaryColor,
      ),
      home: const HomePage(),
    );
  }
}
