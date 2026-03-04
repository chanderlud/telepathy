import 'package:flutter/material.dart' hide Overlay;
import 'package:flutter_svg/flutter_svg.dart';
import 'package:provider/provider.dart';
import 'package:telepathy/controllers/index.dart';
import 'package:telepathy/core/utils/index.dart';
import 'package:telepathy/screens/settings/view.dart';
import 'package:telepathy/src/rust/audio/player.dart';
import 'package:telepathy/src/rust/flutter.dart';
import 'package:telepathy/src/rust/telepathy.dart';

/// A widget with commonly used controls for a call.
class CallControls extends StatefulWidget {
  const CallControls({super.key});

  @override
  State<CallControls> createState() => _CallControlsState();
}

class _CallControlsState extends State<CallControls> {
  late final PeriodicNotifier _notifier;

  @override
  void initState() {
    super.initState();
    _notifier = PeriodicNotifier();
  }

  @override
  void dispose() {
    _notifier.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    final telepathy = context.read<Telepathy>();
    final player = context.read<SoundPlayer>();

    return Column(
      children: [
        const SizedBox(height: 10),
        Consumer<StateController>(builder:
            (BuildContext context, StateController stateController, _) {
          Widget body;

          if (stateController.sessionManagerActive) {
            if (stateController.isCallActive) {
              body = ListenableBuilder(
                  listenable: _notifier,
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
                    style: TextStyle(fontSize: 16, color: Color(0xFFdc2626))),
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
                    : const SizedBox.shrink(),
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
              Selector<AudioSettingsController, double>(
                selector: (context, c) => c.outputVolume,
                builder: (context, outputVolume, child) {
                  final audioSettingsController =
                      context.read<AudioSettingsController>();
                  return Column(
                    crossAxisAlignment: CrossAxisAlignment.start,
                    children: [
                      const Text('Output Volume',
                          style: TextStyle(fontSize: 15)),
                      Slider(
                          value: outputVolume,
                          onChanged: (value) async {
                            await audioSettingsController
                                .updateOutputVolume(value);
                            telepathy.setOutputVolume(decibel: value);
                          },
                          min: -15,
                          max: 15,
                          label: '${outputVolume.toStringAsFixed(2)} db'),
                      const SizedBox(height: 2),
                    ],
                  );
                },
              ),
              Selector<AudioSettingsController, double>(
                selector: (context, c) => c.inputVolume,
                builder: (context, inputVolume, child) {
                  final audioSettingsController =
                      context.read<AudioSettingsController>();
                  return Column(
                    crossAxisAlignment: CrossAxisAlignment.start,
                    children: [
                      const Text('Input Volume',
                          style: TextStyle(fontSize: 15)),
                      Slider(
                          value: inputVolume,
                          onChanged: (value) async {
                            await audioSettingsController
                                .updateInputVolume(value);
                            telepathy.setInputVolume(decibel: value);
                          },
                          min: -15,
                          max: 15,
                          label: '${inputVolume.toStringAsFixed(2)} db'),
                      const SizedBox(height: 2),
                    ],
                  );
                },
              ),
              Selector<AudioSettingsController, double>(
                selector: (context, c) => c.inputSensitivity,
                builder: (context, inputSensitivity, child) {
                  final audioSettingsController =
                      context.read<AudioSettingsController>();
                  return Column(
                    crossAxisAlignment: CrossAxisAlignment.start,
                    children: [
                      const Text('Input Sensitivity',
                          style: TextStyle(fontSize: 15)),
                      Slider(
                          value: inputSensitivity,
                          onChanged: (value) async {
                            await audioSettingsController
                                .updateInputSensitivity(value);
                            telepathy.setRmsThreshold(decimal: value);
                          },
                          min: -16,
                          max: 50,
                          label: '${inputSensitivity.toStringAsFixed(2)} db'),
                    ],
                  );
                },
              ),
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
                  child: Consumer<StateController>(
                      builder: (BuildContext context,
                              StateController stateController, _) =>
                          Row(
                            mainAxisSize: MainAxisSize.min,
                            children: [
                              IconButton(
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
                                      width: 24)),
                              IconButton(
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
                                      width: 28)),
                              IconButton(
                                  onPressed: () {
                                    Navigator.push(
                                        context,
                                        MaterialPageRoute(
                                          builder: (context) => Scaffold(body:
                                              LayoutBuilder(builder:
                                                  (BuildContext context,
                                                      BoxConstraints
                                                          constraints) {
                                            return Title(
                                              title: 'Telepathy | Settings',
                                              color: const Color(0xFF000000),
                                              child: SettingsPage(
                                                constraints: constraints,
                                              ),
                                            );
                                          })),
                                        ));
                                  },
                                  icon: SvgPicture.asset(
                                      'assets/icons/Settings.svg')),
                              const SizedBox(width: 1),
                              IconButton(
                                  onPressed: () async {
                                    if (stateController.activeContact == null) {
                                      return;
                                    }

                                    final networkSettingsController = context
                                        .read<NetworkSettingsController>();

                                    if (!(await screenshareAvailable())) {
                                      if (context.mounted) {
                                        showErrorDialog(
                                            context,
                                            'Screenshare Unavailable',
                                            'ffmpeg must be installed to use the screenshare feature');
                                      }

                                      return;
                                    } else if ((await networkSettingsController
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
                                          contact:
                                              stateController.activeContact!);
                                    } else {
                                      stateController.stopScreenshare(
                                          true, true);
                                    }
                                  },
                                  icon: SvgPicture.asset(
                                      stateController.isSendingScreenshare
                                          ? 'assets/icons/PhoneOff.svg'
                                          : 'assets/icons/Screenshare.svg',
                                      semanticsLabel: 'Screenshare icon')),
                            ],
                          ))),
            ))
      ],
    );
  }
}
