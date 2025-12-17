import 'package:flutter/material.dart' hide Overlay;
import 'package:flutter_svg/flutter_svg.dart';
import 'package:telepathy/controllers/index.dart';
import 'package:telepathy/core/utils/index.dart';
import 'package:telepathy/screens/settings/view.dart';
import 'package:telepathy/src/rust/audio/player.dart';
import 'package:telepathy/src/rust/flutter.dart';
import 'package:telepathy/src/rust/overlay/overlay.dart';
import 'package:telepathy/src/rust/telepathy.dart';

/// A widget with commonly used controls for a call.
class CallControls extends StatelessWidget {
  final Telepathy telepathy;
  final SettingsController settingsController;
  final InterfaceController interfaceController;
  final StateController stateController;
  final StatisticsController statisticsController;
  final SoundPlayer player;
  final PeriodicNotifier notifier;
  final Overlay overlay;
  final AudioDevices audioDevices;

  const CallControls(
      {super.key,
      required this.telepathy,
      required this.settingsController,
      required this.stateController,
      required this.player,
      required this.statisticsController,
      required this.notifier,
      required this.overlay,
      required this.audioDevices,
      required this.interfaceController});

  @override
  Widget build(BuildContext context) {
    return Column(
      children: [
        const SizedBox(height: 10),
        ListenableBuilder(
            listenable: stateController,
            builder: (BuildContext context, Widget? child) {
              Widget body;

              if (stateController.sessionManagerActive) {
                if (stateController.isCallActive) {
                  body = ListenableBuilder(
                      listenable: notifier,
                      builder: (BuildContext context, Widget? child) {
                        return Text(stateController.callDuration,
                            style: const TextStyle(fontSize: 20));
                      });
                } else {
                  body = Text(stateController.status,
                      style: const TextStyle(fontSize: 20));
                }
              } else {
                body = Row(
                  mainAxisAlignment: MainAxisAlignment.center,
                  children: [
                    const SizedBox(width: 15),
                    const Text('Session Manager Inactive',
                        style:
                            TextStyle(fontSize: 16, color: Color(0xFFdc2626))),
                    stateController.sessionManagerRestartable
                        ? const Spacer()
                        : const SizedBox(width: 10),
                    stateController.sessionManagerRestartable
                        ? IconButton(
                            onPressed: () {
                              telepathy.restartManager();
                            },
                            icon: SvgPicture.asset('assets/icons/Restart.svg',
                                colorFilter: const ColorFilter.mode(
                                    Color(0xFFdc2626), BlendMode.srcIn),
                                semanticsLabel: 'Restart session manager'))
                        : Container(),
                    const SizedBox(width: 5),
                  ],
                );
              }

              return SizedBox(
                height: 40,
                child: Center(child: body),
              );
            }),
        Padding(
          padding: const EdgeInsets.only(left: 25, right: 25, top: 20),
          child: Column(
            crossAxisAlignment: CrossAxisAlignment.start,
            children: [
              const Text('Output Volume', style: TextStyle(fontSize: 15)),
              ListenableBuilder(
                  listenable: settingsController,
                  builder: (BuildContext context, Widget? child) {
                    return Slider(
                        value: settingsController.outputVolume,
                        onChanged: (value) async {
                          await settingsController.updateOutputVolume(value);
                          telepathy.setOutputVolume(decibel: value);
                        },
                        min: -15,
                        max: 15,
                        label:
                            '${settingsController.outputVolume.toStringAsFixed(2)} db');
                  }),
              const SizedBox(height: 2),
              const Text('Input Volume', style: TextStyle(fontSize: 15)),
              ListenableBuilder(
                  listenable: settingsController,
                  builder: (BuildContext context, Widget? child) {
                    return Slider(
                        value: settingsController.inputVolume,
                        onChanged: (value) async {
                          await settingsController.updateInputVolume(value);
                          telepathy.setInputVolume(decibel: value);
                        },
                        min: -15,
                        max: 15,
                        label:
                            '${settingsController.inputVolume.toStringAsFixed(2)} db');
                  }),
              const SizedBox(height: 2),
              const Text('Input Sensitivity', style: TextStyle(fontSize: 15)),
              ListenableBuilder(
                  listenable: settingsController,
                  builder: (BuildContext context, Widget? child) {
                    return Slider(
                        value: settingsController.inputSensitivity,
                        onChanged: (value) async {
                          await settingsController
                              .updateInputSensitivity(value);
                          telepathy.setRmsThreshold(decimal: value);
                        },
                        min: -16,
                        max: 50,
                        label:
                            '${settingsController.inputSensitivity.toStringAsFixed(2)} db');
                  }),
            ],
          ),
        ),
        const Spacer(),
        Container(
            decoration: BoxDecoration(
              color: Theme.of(context).colorScheme.secondaryContainer,
              borderRadius: const BorderRadius.only(
                  bottomLeft: Radius.circular(10.0),
                  bottomRight: Radius.circular(10.0)),
            ),
            child: Padding(
              padding: const EdgeInsets.all(5.0),
              child: Center(
                  child: Row(
                mainAxisSize: MainAxisSize.min,
                children: [
                  ListenableBuilder(
                      listenable: stateController,
                      builder: (BuildContext context, Widget? child) {
                        return IconButton(
                            onPressed: () async {
                              if (stateController.isDeafened) {
                                return;
                              }

                              List<int> bytes = stateController.isMuted
                                  ? await readSeaBytes('unmute')
                                  : await readSeaBytes('mute');
                              otherSoundHandle =
                                  await player.play(bytes: bytes);

                              stateController.mute();
                              telepathy.setMuted(
                                  muted: stateController.isMuted);
                            },
                            icon: SvgPicture.asset(
                                stateController.isDeafened |
                                        stateController.isMuted
                                    ? 'assets/icons/MicrophoneOff.svg'
                                    : 'assets/icons/Microphone.svg',
                                width: 24));
                      }),
                  ListenableBuilder(
                      listenable: stateController,
                      builder: (BuildContext context, Widget? child) {
                        return IconButton(
                            onPressed: () async {
                              List<int> bytes = stateController.isDeafened
                                  ? await readSeaBytes('deafen')
                                  : await readSeaBytes('undeafen');
                              otherSoundHandle =
                                  await player.play(bytes: bytes);

                              stateController.deafen();
                              telepathy.setDeafened(
                                  deafened: stateController.isDeafened);

                              if (stateController.isDeafened &&
                                  stateController.isMuted) {
                                telepathy.setMuted(muted: true);
                              } else {
                                telepathy.setMuted(muted: false);
                              }
                            },
                            visualDensity: VisualDensity.comfortable,
                            icon: SvgPicture.asset(
                                stateController.isDeafened
                                    ? 'assets/icons/SpeakerOff.svg'
                                    : 'assets/icons/Speaker.svg',
                                width: 28));
                      }),
                  IconButton(
                      onPressed: () {
                        Navigator.push(
                            context,
                            MaterialPageRoute(
                              builder: (context) => Scaffold(body:
                                  LayoutBuilder(builder: (BuildContext context,
                                      BoxConstraints constraints) {
                                return SettingsPage(
                                  controller: settingsController,
                                  interfaceController: interfaceController,
                                  telepathy: telepathy,
                                  stateController: stateController,
                                  statisticsController: statisticsController,
                                  player: player,
                                  overlay: overlay,
                                  audioDevices: audioDevices,
                                  constraints: constraints,
                                );
                              })),
                            ));
                      },
                      icon: SvgPicture.asset('assets/icons/Settings.svg')),
                  const SizedBox(width: 1),
                  ListenableBuilder(
                      listenable: stateController,
                      builder: (BuildContext context, Widget? child) =>
                          IconButton(
                              onPressed: () async {
                                if (stateController.activeContact == null) {
                                  return;
                                }

                                if (!(await screenshareAvailable())) {
                                  if (context.mounted) {
                                    showErrorDialog(
                                        context,
                                        'Screenshare Unavailable',
                                        'ffmpeg must be installed to use the screenshare feature');
                                  }

                                  return;
                                } else if ((await settingsController
                                        .screenshareConfig
                                        .recordingConfig()) ==
                                    null) {
                                  if (context.mounted) {
                                    showErrorDialog(
                                        context,
                                        'Invalid Configuration',
                                        'An invalid screenshare configuration is active, visit settings to select new options.');
                                  }

                                  return;
                                }

                                if (!stateController.isSendingScreenshare) {
                                  telepathy.startScreenshare(
                                      contact: stateController.activeContact!);
                                } else {
                                  stateController.stopScreenshare(true);
                                }
                              },
                              icon: SvgPicture.asset(
                                  stateController.isSendingScreenshare
                                      ? 'assets/icons/PhoneOff.svg'
                                      : 'assets/icons/Screenshare.svg',
                                  semanticsLabel: 'Screenshare icon'))),
                ],
              )),
            ))
      ],
    );
  }
}
