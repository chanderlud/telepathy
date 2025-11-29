import 'package:file_picker/file_picker.dart';
import 'package:flutter/material.dart';
import 'package:telepathy/audio_level.dart';
import 'package:telepathy/main.dart';
import 'package:telepathy/settings/controller.dart';
import 'package:telepathy/settings/view.dart';
import 'package:telepathy/src/rust/audio/player.dart';
import 'package:telepathy/src/rust/error.dart';
import 'package:telepathy/src/rust/flutter.dart';
import 'package:telepathy/src/rust/telepathy.dart';

class AVSettings extends StatefulWidget {
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
  State<StatefulWidget> createState() => _AVSettingsState();
}

class _AVSettingsState extends State<AVSettings> {
  Capabilities? _capabilities;
  RecordingConfig? _recordingConfig;
  TemporaryConfig? _temporaryConfig;
  bool _loading = false;

  @override
  void initState() {
    super.initState();
    widget.audioDevices.startUpdates();

    var capabilitiesFuture = widget.controller.screenshareConfig.capabilities();
    var recordingConfigFuture =
    widget.controller.screenshareConfig.recordingConfig();

    Future.wait([capabilitiesFuture, recordingConfigFuture])
        .then((List<dynamic> results) {
      _capabilities = results[0];
      _recordingConfig = results[1];
      setState(() {});
    });
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

  void initTemporaryConfig(
      String? encoder, String? device, int? bitrate, int? framerate) {
    setState(() {
      _temporaryConfig = TemporaryConfig(
          encoder: encoder ?? defaultEncoder(),
          device: device ?? defaultDevice(),
          bitrate: bitrate ?? defaultBitrate(),
          framerate: framerate ?? defaultFramerate(),
          height: _recordingConfig?.height());
    });
  }

  String defaultEncoder() {
    return _recordingConfig?.encoder() ??
        _capabilities?.encoders().firstOrNull ??
        'h264';
  }

  String defaultDevice() {
    return _recordingConfig?.device() ?? _capabilities!.devices().first;
  }

  int defaultBitrate() {
    return _recordingConfig?.bitrate() ?? 2000000;
  }

  int defaultFramerate() {
    return _recordingConfig?.framerate() ?? 30;
  }

  @override
  Widget build(BuildContext context) {
    var encoders = _capabilities?.encoders() ?? [];
    var devices = _capabilities?.devices() ?? [];

    double width = widget.constraints.maxWidth < 650
        ? widget.constraints.maxWidth
        : (widget.constraints.maxWidth - 20) / 2;

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
        const Divider(),
        const Text(
          'Screenshare Options',
          style: TextStyle(fontSize: 20),
        ),
        const SizedBox(height: 17),
        Wrap(
          spacing: 20,
          runSpacing: 20,
          children: [
            DropDown(
              label: 'Encoder',
              items: encoders.map((d) => (d, d)).toList(),
              initialSelection:
              _recordingConfig?.encoder() ?? encoders.firstOrNull,
              onSelected: (String? value) {
                if (value == null) {
                  return;
                } else if (_temporaryConfig == null) {
                  initTemporaryConfig(value, null, null, null);
                } else {
                  setState(() {
                    _temporaryConfig!.encoder = value;
                  });
                }
              },
              width: width,
            ),
            DropDown(
              label: 'Capture Device',
              items: devices.map((d) => (d, d)).toList(),
              initialSelection:
              _recordingConfig?.device() ?? devices.firstOrNull,
              onSelected: (String? value) {
                if (value == null) {
                  return;
                } else if (_temporaryConfig == null) {
                  initTemporaryConfig(null, value, null, null);
                } else {
                  setState(() {
                    _temporaryConfig!.device = value;
                  });
                }
              },
              width: width,
            )
          ],
        ),
        const SizedBox(height: 20),
        Button(
            text: _loading ? 'Verifying' : 'Save',
            disabled: _temporaryConfig == null || _loading,
            onPressed: () async {
              try {
                if (_loading) return;

                setState(() {
                  _loading = true;
                });

                await widget.controller.screenshareConfig.updateRecordingConfig(
                    encoder: _temporaryConfig!.encoder,
                    device: _temporaryConfig!.device,
                    bitrate: _temporaryConfig!.bitrate,
                    framerate: _temporaryConfig!.framerate,
                    height: _temporaryConfig!.height);

                widget.controller.saveScreenshareConfig();

                setState(() {
                  _temporaryConfig = null;
                  _loading = false;
                });
              } on DartError catch (e) {
                setState(() {
                  _loading = false;
                });
                if (!context.mounted) return;
                showErrorDialog(
                    context, 'Error in Encoder Selection', e.message);
              }
            }),
      ],
    );
  }
}

class TemporaryConfig {
  String encoder;
  String device;
  int bitrate;
  int framerate;
  int? height;

  TemporaryConfig(
      {required this.encoder,
        required this.device,
        required this.bitrate,
        required this.framerate,
        this.height});
}