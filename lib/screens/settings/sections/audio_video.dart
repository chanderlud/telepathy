import 'package:flutter/material.dart';
import 'package:telepathy/controllers/index.dart';
import 'package:telepathy/screens/settings/sections/audio_settings.dart';
import 'package:telepathy/screens/settings/sections/screenshare_settings.dart';
import 'package:telepathy/src/rust/audio/player.dart';
import 'package:telepathy/src/rust/telepathy.dart';

class AVSettings extends StatelessWidget {
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
  Widget build(BuildContext context) {
    return Column(
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        AudioSettings(
          controller: controller,
          telepathy: telepathy,
          stateController: stateController,
          player: player,
          statisticsController: statisticsController,
          constraints: constraints,
          audioDevices: audioDevices,
        ),
        const SizedBox(height: 20),
        const Divider(),
        const SizedBox(height: 20),
        ScreenshareSettings(
          controller: controller,
          constraints: constraints,
        ),
      ],
    );
  }
}
