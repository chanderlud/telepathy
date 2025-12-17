import 'package:file_picker/file_picker.dart';
import 'package:flutter/material.dart';
import 'package:telepathy/controllers/index.dart';
import 'package:telepathy/core/utils/index.dart';
import 'package:telepathy/src/rust/audio/player.dart';
import 'package:telepathy/src/rust/error.dart';
import 'package:telepathy/src/rust/telepathy.dart';
import 'package:telepathy/widgets/common/index.dart';

class AudioSettings extends StatefulWidget {
  final SettingsController controller;
  final Telepathy telepathy;
  final StateController stateController;
  final StatisticsController statisticsController;
  final SoundPlayer player;
  final BoxConstraints constraints;
  final AudioDevices audioDevices;

  const AudioSettings(
      {super.key,
      required this.controller,
      required this.telepathy,
      required this.stateController,
      required this.player,
      required this.statisticsController,
      required this.constraints,
      required this.audioDevices});

  @override
  State<StatefulWidget> createState() => _AudioSettingsState();
}

class _AudioSettingsState extends State<AudioSettings> {
  @override
  void initState() {
    super.initState();
    widget.audioDevices.startUpdates();
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

  @override
  Widget build(BuildContext context) {
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
        ListenableBuilder(
          listenable: widget.controller,
          builder: (BuildContext context, Widget? child) {
            final values = widget.controller.codecConfig.toValues();
            final bool codecEnabled = values.$1;
            final bool codecVbr = values.$2;
            final double residualBits = values.$3.clamp(1.0, 8.0).toDouble();

            return Column(
              crossAxisAlignment: CrossAxisAlignment.start,
              children: [
                const SizedBox(height: 12),
                Row(
                  mainAxisAlignment: MainAxisAlignment.spaceBetween,
                  children: [
                    const Text('Enable Codec', style: TextStyle(fontSize: 18)),
                    CustomSwitch(
                      value: codecEnabled,
                      onChanged: (enabled) {
                        widget.controller.updateCodecEnabled(enabled);
                      },
                    ),
                  ],
                ),
                if (codecEnabled) ...[
                  const SizedBox(height: 12),
                  Row(
                    mainAxisAlignment: MainAxisAlignment.spaceBetween,
                    children: [
                      const Text(
                        'Variable Bitrate (VBR)',
                        style: TextStyle(fontSize: 18),
                      ),
                      CustomSwitch(
                        value: codecVbr,
                        onChanged: (vbr) {
                          widget.controller.updateCodecVbr(vbr);
                        },
                      ),
                    ],
                  ),
                  const SizedBox(height: 12),
                  const Text(
                    'Residual Bits',
                    style: TextStyle(fontSize: 18),
                  ),
                  Slider(
                    min: 1.0,
                    max: 8.0,
                    value: residualBits,
                    label: residualBits.toStringAsFixed(1),
                    onChanged: (value) {
                      widget.controller.updateCodecResidualBits(value);
                    },
                  ),
                ],
              ],
            );
          },
        ),
      ],
    );
  }
}
