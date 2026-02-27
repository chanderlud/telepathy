import 'package:flutter/material.dart';
import 'package:telepathy/screens/settings/sections/audio_settings.dart';
import 'package:telepathy/screens/settings/sections/screenshare_settings.dart';

class AVSettings extends StatelessWidget {
  final BoxConstraints constraints;

  const AVSettings({super.key, required this.constraints});

  @override
  Widget build(BuildContext context) {
    return Column(
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        AudioSettings(
          constraints: constraints,
        ),
        const SizedBox(height: 20),
        const Divider(),
        const SizedBox(height: 20),
        ScreenshareSettings(
          constraints: constraints,
        ),
      ],
    );
  }
}
