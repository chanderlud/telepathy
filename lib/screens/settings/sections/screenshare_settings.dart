import 'package:collection/collection.dart';
import 'package:flutter/material.dart';
import 'package:telepathy/core/constants/audio_constants.dart'
    as audio_constants;
import 'package:telepathy/controllers/index.dart';
import 'package:telepathy/core/utils/index.dart';
import 'package:telepathy/models/index.dart';
import 'package:telepathy/src/rust/error.dart';
import 'package:telepathy/src/rust/flutter.dart';
import 'package:telepathy/widgets/common/index.dart';

class ScreenshareSettings extends StatefulWidget {
  final SettingsController controller;
  final BoxConstraints constraints;

  const ScreenshareSettings({
    super.key,
    required this.controller,
    required this.constraints,
  });

  @override
  State<ScreenshareSettings> createState() => _ScreenshareSettingsState();
}

class _ScreenshareSettingsState extends State<ScreenshareSettings> {
  Capabilities? _capabilities;
  RecordingConfig? _recordingConfig;
  TemporaryConfig? _temporaryConfig;
  bool _loading = false;

  @override
  void initState() {
    super.initState();

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

    int currentBitrate = _temporaryConfig?.bitrate ??
        _recordingConfig?.bitrate() ??
        defaultBitrate();

    double bitrateValue = currentBitrate
        .clamp(audio_constants.minBitrate, audio_constants.maxBitrate)
        .toDouble();

    int currentFramerate = _temporaryConfig?.framerate ??
        _recordingConfig?.framerate() ??
        defaultFramerate();

    return Column(
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
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
        DropDown(
          label: 'Framerate (FPS)',
          items: const [
            ('15', '15 FPS'),
            ('30', '30 FPS'),
            ('60', '60 FPS'),
            ('120', '120 FPS'),
          ],
          initialSelection: currentFramerate.toString(),
          onSelected: (String? value) {
            if (value == null) return;

            final int fps = int.tryParse(value) ?? defaultFramerate();

            if (_temporaryConfig == null) {
              initTemporaryConfig(null, null, null, fps);
            } else {
              setState(() {
                _temporaryConfig!.framerate = fps;
              });
            }
          },
          width: width,
        ),
        const SizedBox(height: 20),
        Column(
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            const Text(
              'Bitrate',
              style: TextStyle(fontSize: 16),
            ),
            Slider(
              min: audio_constants.minBitrate.toDouble(),
              max: audio_constants.maxBitrate.toDouble(),
              value: bitrateValue,
              label: '${(bitrateValue / 1000000).round()}mbps',
              onChanged: (double value) {
                final int newBitrate = value.round();

                if (_temporaryConfig == null) {
                  initTemporaryConfig(null, null, newBitrate, null);
                } else {
                  setState(() {
                    _temporaryConfig!.bitrate = newBitrate;
                  });
                }
              },
            ),
          ],
        ),
        const SizedBox(height: 20),
        Button(
            text: _loading ? 'Verifying' : 'Save',
            disabled: _temporaryConfig == null || _loading,
            onPressed: () async {
              if (_loading) return;

              setState(() {
                _loading = true;
              });

              try {
                await widget.controller.screenshareConfig.updateRecordingConfig(
                    encoder: _temporaryConfig!.encoder,
                    device: _temporaryConfig!.device,
                    bitrate: _temporaryConfig!.bitrate,
                    framerate: _temporaryConfig!.framerate,
                    height: _temporaryConfig!.height);
              } on DartError catch (e) {
                setState(() {
                  _loading = false;
                });

                if (context.mounted) {
                  showErrorDialog(
                      context, 'Error in Encoder Selection', e.message);
                }

                return;
              }

              widget.controller.saveScreenshareConfig();
              setState(() {
                _temporaryConfig = null;
                _loading = false;
              });
            }),
      ],
    );
  }
}
