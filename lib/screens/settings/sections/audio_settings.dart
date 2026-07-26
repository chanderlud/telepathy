import 'package:collection/collection.dart';
import 'package:file_picker/file_picker.dart';
import 'package:flutter/foundation.dart' show kIsWeb;
import 'package:flutter/material.dart';
import 'package:provider/provider.dart';
import 'package:telepathy/controllers/index.dart';
import 'package:telepathy/core/utils/index.dart';
import 'package:telepathy/core/rust/player.dart';
import 'package:telepathy/core/rust/types.dart';
import 'package:telepathy/core/rust/lib.dart';
import 'package:telepathy/core/rust/flutter.dart';
import 'package:telepathy/widgets/common/index.dart';

class AudioSettings extends StatefulWidget {
  final BoxConstraints constraints;

  const AudioSettings({super.key, required this.constraints});

  @override
  State<StatefulWidget> createState() => _AudioSettingsState();
}

class _AudioSettingsState extends State<AudioSettings> {
  late final AudioDevices _audioDevices;
  bool testCooldown = false;

  @override
  void initState() {
    super.initState();
    _audioDevices = context.read<AudioDevices>();
    _audioDevices.startUpdates();
  }

  Widget _settingsSwitchRow({
    required String label,
    required Widget trailing,
  }) {
    return Row(
      mainAxisAlignment: MainAxisAlignment.spaceBetween,
      mainAxisSize: MainAxisSize.max,
      children: [
        Text(label, style: const TextStyle(fontSize: 18)),
        trailing,
      ],
    );
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
    final audioSettingsController = context.read<AudioSettingsController>();

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
            if (!kIsWeb) {
              return Selector2<AudioDevices, AudioSettingsController,
                  _DeviceDropdownState>(
                selector: (context, audioDevices, audioSettingsController) =>
                    _DeviceDropdownState(
                  inputDevices:
                      List<AudioDevice>.unmodifiable(audioDevices.inputDevices),
                  outputDevices: List<AudioDevice>.unmodifiable(
                      audioDevices.outputDevices),
                  selectedInputDevice: audioSettingsController.inputDeviceId,
                  selectedOutputDevice: audioSettingsController.outputDeviceId,
                  hasLoadedDevices: audioDevices.hasLoadedDevices,
                ),
                builder: (BuildContext context, _DeviceDropdownState state, _) {
                  final inputInitialSelection = state.selectedInputDevice ?? '';
                  final outputInitialSelection =
                      state.selectedOutputDevice ?? '';
                  final inputUnavailable = state.hasLoadedDevices &&
                      inputInitialSelection.isNotEmpty &&
                      !state.inputDevices
                          .any((device) => device.id == inputInitialSelection);
                  final outputUnavailable = state.hasLoadedDevices &&
                      outputInitialSelection.isNotEmpty &&
                      !state.outputDevices
                          .any((device) => device.id == outputInitialSelection);

                  final double width = widget.constraints.maxWidth < 650
                      ? widget.constraints.maxWidth
                      : (widget.constraints.maxWidth - 20) / 2;

                  return Wrap(
                    spacing: 20,
                    runSpacing: 20,
                    children: [
                      SizedBox(
                        width: width,
                        child: Column(
                          crossAxisAlignment: CrossAxisAlignment.start,
                          children: [
                            if (inputUnavailable) ...[
                              _unavailableDeviceWarning(
                                'Selected input device is unavailable',
                              ),
                              const SizedBox(height: 6),
                            ],
                            DropDown(
                              key: ValueKey(Object.hashAll([
                                inputInitialSelection,
                                ...state.inputDevices
                                    .map((device) => device.id),
                              ])),
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
                              width: width,
                            ),
                          ],
                        ),
                      ),
                      SizedBox(
                        width: width,
                        child: Column(
                          crossAxisAlignment: CrossAxisAlignment.start,
                          children: [
                            if (outputUnavailable) ...[
                              _unavailableDeviceWarning(
                                'Selected output device is unavailable',
                              ),
                              const SizedBox(height: 6),
                            ],
                            DropDown(
                              key: ValueKey(Object.hashAll([
                                outputInitialSelection,
                                ...state.outputDevices
                                    .map((device) => device.id),
                              ])),
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
                            ),
                          ],
                        ),
                      ),
                    ],
                  );
                },
              );
            }

            return const SizedBox.shrink();
          },
        ),
        const SizedBox(height: 20),
        Row(children: [
          Selector<StateController, (bool, bool)>(
            selector: (context, controller) =>
                (controller.inAudioTest, controller.hasLiveCall),
            builder: (BuildContext context, state, _) {
              final (inAudioTest, hasLiveCall) = state;
              final stateController = context.read<StateController>();
              return Button(
                text: inAudioTest ? 'End Test' : 'Sound Test',
                width: 80,
                height: 25,
                disabled: hasLiveCall,
                onPressed: () async {
                  // 100ms debounce for safety
                  if (testCooldown) {
                    return;
                  } else {
                    setState(() {
                      testCooldown = true;
                    });
                    Future.delayed(const Duration(milliseconds: 100), () {
                      if (!mounted) return;
                      setState(() {
                        testCooldown = false;
                      });
                    });
                  }

                  if (inAudioTest) {
                    await telepathy.endCall();
                    stateController.setInAudioTest(false);
                  } else {
                    try {
                      await stateController.runAudioTest(telepathy.audioTest);
                    } on DartError catch (e) {
                      if (!context.mounted) return;
                      showErrorDialog(
                          context, 'Error in Audio Test', e.message);
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
                        // save denoise option
                        audioSettingsController.updateUseDenoise(true);
                        // save denoise model
                        audioSettingsController.setDenoiseModel(value);
                        // set denoise to true
                        telepathy.setDenoise(denoise: true);
                        // set denoise model — pass null for Vanilla (built-in default)
                        updateDenoiseModel(
                            value == 'Vanilla' ? null : value, telepathy);
                      }
                    });
              },
            ),
          ],
        ),
        const SizedBox(height: 5),
        _settingsSwitchRow(
          label: 'Play Custom Ringtones',
          trailing: Selector<PreferencesController, bool>(
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

                  FilePickerResult? result = await FilePicker.pickFiles(
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
        _settingsSwitchRow(
          label: 'Enable Efficiency Mode',
          trailing: Selector<PreferencesController, bool>(
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
        ),
        Consumer<NetworkSettingsController>(
          builder: (BuildContext context,
              NetworkSettingsController networkSettingsController, _) {
            final values = networkSettingsController.codecConfig.toValues();
            final bool codecEnabled = values.$1;
            final bool codecVbr = values.$2;
            final double residualBits = values.$3.clamp(2.0, 8.0).toDouble();

            return Column(
              crossAxisAlignment: CrossAxisAlignment.start,
              children: [
                const SizedBox(height: 12),
                _settingsSwitchRow(
                  label: 'Enable Codec',
                  trailing: CustomSwitch(
                    value: codecEnabled,
                    onChanged: (enabled) {
                      networkSettingsController.updateCodecEnabled(enabled);
                    },
                  ),
                ),
                if (codecEnabled) ...[
                  const SizedBox(height: 12),
                  _settingsSwitchRow(
                    label: 'Variable Bitrate (VBR)',
                    trailing: CustomSwitch(
                      value: codecVbr,
                      onChanged: (vbr) {
                        networkSettingsController.updateCodecVbr(vbr);
                      },
                    ),
                  ),
                  const SizedBox(height: 12),
                  const Text(
                    'Residual Bits',
                    style: TextStyle(fontSize: 18),
                  ),
                  Slider(
                    min: 2.0,
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

Widget _unavailableDeviceWarning(String message) {
  return Container(
    width: double.infinity,
    padding: const EdgeInsets.symmetric(horizontal: 8, vertical: 5),
    decoration: BoxDecoration(
      color: Colors.amber.withValues(alpha: 0.16),
      borderRadius: BorderRadius.circular(12),
    ),
    child: Row(
      mainAxisSize: MainAxisSize.min,
      children: [
        const Icon(Icons.warning_amber_rounded, color: Colors.amber, size: 18),
        const SizedBox(width: 5),
        Flexible(
          child: Text(
            message,
            style: TextStyle(color: Colors.amber[800], fontSize: 13),
          ),
        ),
      ],
    ),
  );
}

class _DeviceDropdownState {
  final List<AudioDevice> inputDevices;
  final List<AudioDevice> outputDevices;
  final String? selectedInputDevice;
  final String? selectedOutputDevice;
  final bool hasLoadedDevices;

  _DeviceDropdownState({
    required this.inputDevices,
    required this.outputDevices,
    required this.selectedInputDevice,
    required this.selectedOutputDevice,
    required this.hasLoadedDevices,
  });

  @override
  bool operator ==(Object other) =>
      identical(this, other) ||
      other is _DeviceDropdownState &&
          runtimeType == other.runtimeType &&
          const ListEquality().equals(inputDevices, other.inputDevices) &&
          const ListEquality().equals(outputDevices, other.outputDevices) &&
          selectedInputDevice == other.selectedInputDevice &&
          selectedOutputDevice == other.selectedOutputDevice &&
          hasLoadedDevices == other.hasLoadedDevices;

  @override
  int get hashCode => Object.hash(
        const ListEquality().hash(inputDevices),
        const ListEquality().hash(outputDevices),
        selectedInputDevice,
        selectedOutputDevice,
        hasLoadedDevices,
      );
}
