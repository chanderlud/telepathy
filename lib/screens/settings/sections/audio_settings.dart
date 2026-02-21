import 'package:collection/collection.dart';
import 'package:file_picker/file_picker.dart';
import 'package:flutter/material.dart';
import 'package:provider/provider.dart';
import 'package:telepathy/controllers/index.dart';
import 'package:telepathy/core/utils/index.dart';
import 'package:telepathy/src/rust/audio/player.dart';
import 'package:telepathy/src/rust/error.dart';
import 'package:telepathy/src/rust/telepathy.dart';
import 'package:telepathy/widgets/common/index.dart';

class AudioSettings extends StatefulWidget {
  final BoxConstraints constraints;

  const AudioSettings({super.key, required this.constraints});

  @override
  State<StatefulWidget> createState() => _AudioSettingsState();
}

class _AudioSettingsState extends State<AudioSettings> {
  late final AudioDevices _audioDevices;

  @override
  void initState() {
    super.initState();
    _audioDevices = context.read<AudioDevices>();
    _audioDevices.startUpdates();
  }

  @override
  void activate() {
    super.activate();
    _audioDevices.startUpdates();
  }

  @override
  void deactivate() {
    _audioDevices.pauseUpdates();
    super.deactivate();
  }

  @override
  Widget build(BuildContext context) {
    final telepathy = context.read<Telepathy>();
    final player = context.read<SoundPlayer>();

    return Column(
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        const Text(
          'Audio Options',
          style: TextStyle(fontSize: 20),
        ),
        const SizedBox(height: 17),
        Selector<StateController, bool>(
          selector: (context, controller) => controller.blockAudioChanges,
          builder: (BuildContext context, bool blockAudioChanges, _) {
            return Selector2<AudioDevices, AudioSettingsController,
                _DeviceDropdownState>(
              selector: (context, audioDevices, audioSettingsController) =>
                  _DeviceDropdownState(
                inputDevices:
                    List<AudioDevice>.unmodifiable(audioDevices.inputDevices),
                outputDevices:
                    List<AudioDevice>.unmodifiable(audioDevices.outputDevices),
                selectedInputDevice: audioSettingsController.inputDeviceId,
                selectedOutputDevice: audioSettingsController.outputDeviceId,
              ),
              builder: (BuildContext context, _DeviceDropdownState state, _) {
                final audioSettingsController =
                    context.read<AudioSettingsController>();

                String inputInitialSelection;
                if (state.selectedInputDevice == null) {
                  inputInitialSelection = '';
                } else if (state.inputDevices.firstWhereOrNull(
                        (d) => d.id == state.selectedInputDevice) !=
                    null) {
                  inputInitialSelection = state.selectedInputDevice!;
                } else {
                  inputInitialSelection = '';
                }

                String outputInitialSelection;
                if (state.selectedOutputDevice == null) {
                  outputInitialSelection = '';
                } else if (state.outputDevices.firstWhereOrNull(
                        (d) => d.id == state.selectedOutputDevice) !=
                    null) {
                  outputInitialSelection = state.selectedOutputDevice!;
                } else {
                  outputInitialSelection = '';
                }

                final double width = widget.constraints.maxWidth < 650
                    ? widget.constraints.maxWidth
                    : (widget.constraints.maxWidth - 20) / 2;

                return Wrap(
                  spacing: 20,
                  runSpacing: 20,
                  children: [
                    DropDown(
                        label: 'Input Device',
                        items: state.inputDevices
                            .map((d) => (d.id, d.name))
                            .toList(),
                        initialSelection: inputInitialSelection,
                        enabled: !blockAudioChanges,
                        onSelected: (String? id) {
                          if (id == '') id = null;
                          audioSettingsController.updateInputDevice(id);
                          telepathy.setInputDevice(deviceId: id);
                        },
                        width: width),
                    DropDown(
                      label: 'Output Device',
                      items: state.outputDevices
                          .map((d) => (d.id, d.name))
                          .toList(),
                      initialSelection: outputInitialSelection,
                      enabled: !blockAudioChanges,
                      onSelected: (String? id) {
                        if (id == '') id = null;
                        audioSettingsController.updateOutputDevice(id);
                        telepathy.setOutputDevice(deviceId: id);
                        player.updateOutputDevice(deviceId: id);
                      },
                      width: width,
                    )
                  ],
                );
              },
            );
          },
        ),
        const SizedBox(height: 20),
        Row(children: [
          Selector<StateController, (bool, bool)>(
            selector: (context, controller) =>
                (controller.inAudioTest, controller.isCallActive),
            builder: (BuildContext context, state, _) {
              final (inAudioTest, isCallActive) = state;
              final stateController = context.read<StateController>();
              return Button(
                text: inAudioTest ? 'End Test' : 'Sound Test',
                width: 80,
                height: 25,
                disabled: isCallActive,
                onPressed: () async {
                  if (inAudioTest) {
                    stateController.setInAudioTest();
                    telepathy.endCall();
                  } else {
                    stateController.setInAudioTest();
                    try {
                      await telepathy.audioTest();
                    } on DartError catch (e) {
                      if (!context.mounted) return;
                      showErrorDialog(
                          context, 'Error in Audio Test', e.message);
                      stateController.setInAudioTest();
                    }
                  }
                },
              );
            },
          ),
          const SizedBox(width: 20),
          Selector<StatisticsController, double>(
            selector: (context, controller) => controller.inputLevel,
            builder: (BuildContext context, double inputLevel, _) {
              return AudioLevel(
                  level: inputLevel,
                  numRectangles: (widget.constraints.maxWidth - 145) ~/ 13.5);
            },
          ),
        ]),
        const SizedBox(height: 20),
        Row(
          mainAxisAlignment: MainAxisAlignment.spaceBetween,
          mainAxisSize: MainAxisSize.max,
          children: [
            const Text('Noise Suppression', style: TextStyle(fontSize: 18)),
            Selector<AudioSettingsController, (bool, String?)>(
              selector: (context, controller) =>
                  (controller.useDenoise, controller.denoiseModel),
              builder: (BuildContext context, state, _) {
                final (useDenoise, denoiseModel) = state;
                final audioSettingsController =
                    context.read<AudioSettingsController>();
                return DropDown(
                    items: const [
                      ('Off', 'Off'),
                      ('Vanilla', 'Vanilla'),
                      ('Hogwash', 'Hogwash')
                    ],
                    initialSelection:
                        useDenoise ? (denoiseModel ?? 'Vanilla') : 'Off',
                    onSelected: (String? value) {
                      if (value == 'Off') {
                        // save denoise option
                        audioSettingsController.updateUseDenoise(false);
                        // set denoise to false
                        telepathy.setDenoise(denoise: false);
                      } else {
                        if (value == 'Vanilla') {
                          value = null;
                        }

                        // save denoise option
                        audioSettingsController.updateUseDenoise(true);
                        // save denoise model
                        audioSettingsController.setDenoiseModel(value);
                        // set denoise to true
                        telepathy.setDenoise(denoise: true);
                        // set denoise model
                        updateDenoiseModel(value, telepathy);
                      }
                    });
              },
            ),
          ],
        ),
        const SizedBox(height: 5),
        Row(
          mainAxisAlignment: MainAxisAlignment.spaceBetween,
          mainAxisSize: MainAxisSize.max,
          children: [
            const Text('Play Custom Ringtones', style: TextStyle(fontSize: 18)),
            Selector<PreferencesController, bool>(
              selector: (context, controller) => controller.playCustomRingtones,
              builder: (BuildContext context, bool playCustomRingtones, _) {
                return CustomSwitch(
                    value: playCustomRingtones,
                    onChanged: (play) {
                      context
                          .read<PreferencesController>()
                          .updatePlayCustomRingtones(play);
                      telepathy.setPlayCustomRingtones(play: play);
                    });
              },
            ),
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
                  final preferencesController =
                      context.read<PreferencesController>();

                  FilePickerResult? result =
                      await FilePicker.platform.pickFiles(
                    type: FileType.custom,
                    allowedExtensions: ['wav'],
                  );

                  if (result != null) {
                    String? path = result.files.single.path;
                    preferencesController.updateCustomRingtoneFile(path);
                    telepathy.setSendCustomRingtone(send: true);
                    loadRingtone(path: path!);
                  } else {
                    preferencesController.updateCustomRingtoneFile(null);
                    telepathy.setSendCustomRingtone(send: false);
                  }
                }),
            Selector<PreferencesController, String?>(
              selector: (context, controller) => controller.customRingtoneFile,
              builder: (BuildContext context, String? customRingtoneFile, _) {
                return Text(customRingtoneFile ?? '',
                    style: const TextStyle(fontSize: 16));
              },
            ),
          ],
        ),
        const SizedBox(height: 20),
        const Text('Sound Effect Volume', style: TextStyle(fontSize: 16)),
        Selector<AudioSettingsController, double>(
          selector: (context, controller) => controller.soundVolume,
          builder: (BuildContext context, double soundVolume, _) {
            final audioSettingsController =
                context.read<AudioSettingsController>();
            return Slider(
                value: soundVolume,
                onChanged: (value) {
                  audioSettingsController.updateSoundVolume(value);
                  player.updateOutputVolume(volume: value);
                },
                min: -20,
                max: 20,
                label: '${soundVolume.toStringAsFixed(2)} db');
          },
        ),
        const SizedBox(height: 5),
        Row(
          mainAxisAlignment: MainAxisAlignment.spaceBetween,
          mainAxisSize: MainAxisSize.max,
          children: [
            const Text('Enable Efficiency Mode',
                style: TextStyle(fontSize: 18)),
            Selector<PreferencesController, bool>(
              selector: (context, controller) => controller.efficiencyMode,
              builder: (BuildContext context, bool efficiencyMode, _) {
                return CustomSwitch(
                    value: efficiencyMode,
                    onChanged: (enabled) {
                      context
                          .read<PreferencesController>()
                          .updateEfficiencyMode(enabled);
                      telepathy.setEfficiencyMode(enabled: enabled);
                    });
              },
            ),
          ],
        ),
        Consumer<NetworkSettingsController>(
          builder: (BuildContext context,
              NetworkSettingsController networkSettingsController, _) {
            final values = networkSettingsController.codecConfig.toValues();
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
                        networkSettingsController.updateCodecEnabled(enabled);
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
                          networkSettingsController.updateCodecVbr(vbr);
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
                      networkSettingsController.updateCodecResidualBits(value);
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

class _DeviceDropdownState {
  final List<AudioDevice> inputDevices;
  final List<AudioDevice> outputDevices;
  final String? selectedInputDevice;
  final String? selectedOutputDevice;

  _DeviceDropdownState({
    required this.inputDevices,
    required this.outputDevices,
    required this.selectedInputDevice,
    required this.selectedOutputDevice,
  });

  @override
  bool operator ==(Object other) =>
      identical(this, other) ||
      other is _DeviceDropdownState &&
          runtimeType == other.runtimeType &&
          const ListEquality().equals(inputDevices, other.inputDevices) &&
          const ListEquality().equals(outputDevices, other.outputDevices) &&
          selectedInputDevice == other.selectedInputDevice &&
          selectedOutputDevice == other.selectedOutputDevice;

  @override
  int get hashCode => Object.hash(
        const ListEquality().hash(inputDevices),
        const ListEquality().hash(outputDevices),
        selectedInputDevice,
        selectedOutputDevice,
      );
}
