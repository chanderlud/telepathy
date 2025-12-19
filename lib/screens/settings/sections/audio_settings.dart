import 'package:collection/collection.dart';
import 'package:file_picker/file_picker.dart';
import 'package:flutter/material.dart';
import 'package:telepathy/controllers/index.dart';
import 'package:telepathy/core/utils/index.dart';
import 'package:telepathy/src/rust/audio/player.dart';
import 'package:telepathy/src/rust/error.dart';
import 'package:telepathy/src/rust/telepathy.dart';
import 'package:telepathy/widgets/common/index.dart';

class AudioSettings extends StatefulWidget {
  final AudioSettingsController audioSettingsController;
  final PreferencesController preferencesController;
  final NetworkSettingsController networkSettingsController;
  final Telepathy telepathy;
  final StateController stateController;
  final StatisticsController statisticsController;
  final SoundPlayer player;
  final BoxConstraints constraints;
  final AudioDevices audioDevices;

  const AudioSettings(
      {super.key,
      required this.audioSettingsController,
      required this.preferencesController,
      required this.networkSettingsController,
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
  late Listenable _deviceDropdownListenable;

  @override
  void initState() {
    super.initState();
    _deviceDropdownListenable =
        Listenable.merge([widget.audioDevices, widget.audioSettingsController]);
    widget.audioDevices.startUpdates();
  }

  @override
  void didUpdateWidget(covariant AudioSettings oldWidget) {
    super.didUpdateWidget(oldWidget);

    if (oldWidget.audioDevices != widget.audioDevices ||
        oldWidget.audioSettingsController != widget.audioSettingsController) {
      _deviceDropdownListenable = Listenable.merge(
          [widget.audioDevices, widget.audioSettingsController]);
    }
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
        Selector<Listenable, _DeviceDropdownState>(
          listenable: _deviceDropdownListenable,
          selector: (_) => _DeviceDropdownState(
            inputDevices:
                List<String>.unmodifiable(widget.audioDevices.inputDevices),
            outputDevices:
                List<String>.unmodifiable(widget.audioDevices.outputDevices),
            selectedInputDevice: widget.audioSettingsController.inputDevice,
            selectedOutputDevice: widget.audioSettingsController.outputDevice,
          ),
          builder: (BuildContext context, _DeviceDropdownState state) {
            String inputInitialSelection;
            if (state.selectedInputDevice == null) {
              inputInitialSelection = 'Default';
            } else if (state.inputDevices.contains(state.selectedInputDevice)) {
              inputInitialSelection = state.selectedInputDevice!;
            } else {
              inputInitialSelection = 'Default';
            }

            String outputInitialSelection;
            if (state.selectedOutputDevice == null) {
              outputInitialSelection = 'Default';
            } else if (state.outputDevices.contains(state.selectedOutputDevice)) {
              outputInitialSelection = state.selectedOutputDevice!;
            } else {
              outputInitialSelection = 'Default';
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
                    items: state.inputDevices.map((d) => (d, d)).toList(),
                    initialSelection: inputInitialSelection,
                    onSelected: (String? value) {
                      if (value == 'Default') value = null;
                      widget.audioSettingsController.updateInputDevice(value);
                      widget.telepathy.setInputDevice(device: value);
                    },
                    width: width),
                DropDown(
                  label: 'Output Device',
                  items: state.outputDevices.map((d) => (d, d)).toList(),
                  initialSelection: outputInitialSelection,
                  onSelected: (String? value) {
                    if (value == 'Default') value = null;
                    widget.audioSettingsController.updateOutputDevice(value);
                    widget.telepathy.setOutputDevice(device: value);
                    widget.player.updateOutputDevice(name: value);
                  },
                  width: width,
                )
              ],
            );
          },
        ),
        const SizedBox(height: 20),
        Row(children: [
          Selector<StateController, (bool, bool)>(
            listenable: widget.stateController,
            selector: (controller) =>
                (controller.inAudioTest, controller.isCallActive),
            builder: (BuildContext context, state) {
              final (inAudioTest, isCallActive) = state;
              return Button(
                text: inAudioTest ? 'End Test' : 'Sound Test',
                width: 80,
                height: 25,
                disabled: isCallActive,
                onPressed: () async {
                  if (inAudioTest) {
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
            },
          ),
          const SizedBox(width: 20),
          Selector<StatisticsController, double>(
            listenable: widget.statisticsController,
            selector: (controller) => controller.inputLevel,
            builder: (BuildContext context, double inputLevel) {
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
              listenable: widget.audioSettingsController,
              selector: (controller) =>
                  (controller.useDenoise, controller.denoiseModel),
              builder: (BuildContext context, state) {
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
                        widget.audioSettingsController.updateUseDenoise(false);
                        // set denoise to false
                        widget.telepathy.setDenoise(denoise: false);
                      } else {
                        if (value == 'Vanilla') {
                          value = null;
                        }

                        // save denoise option
                        widget.audioSettingsController.updateUseDenoise(true);
                        // save denoise model
                        widget.audioSettingsController.setDenoiseModel(value);
                        // set denoise to true
                        widget.telepathy.setDenoise(denoise: true);
                        // set denoise model
                        updateDenoiseModel(value, widget.telepathy);
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
              listenable: widget.preferencesController,
              selector: (controller) => controller.playCustomRingtones,
              builder: (BuildContext context, bool playCustomRingtones) {
                return CustomSwitch(
                    value: playCustomRingtones,
                    onChanged: (play) {
                      widget.preferencesController
                          .updatePlayCustomRingtones(play);
                      widget.telepathy.setPlayCustomRingtones(play: play);
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
                  FilePickerResult? result =
                      await FilePicker.platform.pickFiles(
                    type: FileType.custom,
                    allowedExtensions: ['wav'],
                  );

                  if (result != null) {
                    String? path = result.files.single.path;
                    widget.preferencesController.updateCustomRingtoneFile(path);
                    widget.telepathy.setSendCustomRingtone(send: true);
                    loadRingtone(path: path!);
                  } else {
                    widget.preferencesController.updateCustomRingtoneFile(null);
                    widget.telepathy.setSendCustomRingtone(send: false);
                  }
                }),
            Selector<PreferencesController, String?>(
              listenable: widget.preferencesController,
              selector: (controller) => controller.customRingtoneFile,
              builder: (BuildContext context, String? customRingtoneFile) {
                return Text(customRingtoneFile ?? '',
                    style: const TextStyle(fontSize: 16));
              },
            ),
          ],
        ),
        const SizedBox(height: 20),
        const Text('Sound Effect Volume', style: TextStyle(fontSize: 16)),
        Selector<AudioSettingsController, double>(
          listenable: widget.audioSettingsController,
          selector: (controller) => controller.soundVolume,
          builder: (BuildContext context, double soundVolume) {
            return Slider(
                value: soundVolume,
                onChanged: (value) {
                  widget.audioSettingsController.updateSoundVolume(value);
                  widget.player.updateOutputVolume(volume: value);
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
              listenable: widget.preferencesController,
              selector: (controller) => controller.efficiencyMode,
              builder: (BuildContext context, bool efficiencyMode) {
                return CustomSwitch(
                    value: efficiencyMode,
                    onChanged: (enabled) {
                      widget.preferencesController.updateEfficiencyMode(enabled);
                      widget.telepathy.setEfficiencyMode(enabled: enabled);
                    });
              },
            ),
          ],
        ),
        ListenableBuilder(
          listenable: widget.networkSettingsController,
          builder: (BuildContext context, Widget? child) {
            final values =
                widget.networkSettingsController.codecConfig.toValues();
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
                        widget.networkSettingsController
                            .updateCodecEnabled(enabled);
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
                          widget.networkSettingsController.updateCodecVbr(vbr);
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
                      widget.networkSettingsController
                          .updateCodecResidualBits(value);
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
  final List<String> inputDevices;
  final List<String> outputDevices;
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
