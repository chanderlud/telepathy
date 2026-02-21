import 'package:flutter/material.dart' hide Overlay;
import 'package:provider/provider.dart';
import 'package:telepathy/controllers/index.dart';
import 'package:telepathy/screens/settings/sections/overlay/color_picker_dialog.dart';
import 'package:telepathy/screens/settings/sections/overlay/overlay_position_widget.dart';
import 'package:telepathy/src/rust/overlay/overlay.dart';
import 'package:telepathy/widgets/common/index.dart';

class OverlaySettings extends StatefulWidget {
  const OverlaySettings({super.key});

  @override
  OverlaySettingsState createState() => OverlaySettingsState();
}

class OverlaySettingsState extends State<OverlaySettings> {
  bool overlayVisible = false;
  late final Overlay _overlay;
  late final StateController _stateController;
  late final NetworkSettingsController _networkSettingsController;

  @override
  void initState() {
    super.initState();
    _overlay = context.read<Overlay>();
    _stateController = context.read<StateController>();
    _networkSettingsController = context.read<NetworkSettingsController>();
  }

  @override
  void dispose() {
    if (!_stateController.isCallActive) {
      _overlay.hide_();
    }

    _networkSettingsController.saveOverlayConfig();

    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    (int, int) size = _overlay.screenResolution();

    return Column(
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        Row(
          children: [
            Button(
              text: overlayVisible ? 'Hide overlay' : 'Show overlay',
              onPressed: () {
                if (_stateController.isCallActive ||
                    !_networkSettingsController.overlayConfig.enabled) {
                  return;
                } else if (overlayVisible) {
                  _overlay.hide_();
                } else {
                  _overlay.show_();
                }

                setState(() {
                  overlayVisible = !overlayVisible;
                });
              },
              disabled: _stateController.isCallActive ||
                  !_networkSettingsController.overlayConfig.enabled,
              width: 90,
              height: 25,
            ),
            const SizedBox(width: 20),
            Button(
              text: _networkSettingsController.overlayConfig.enabled
                  ? 'Disable overlay'
                  : 'Enable overlay',
              onPressed: () async {
                if (_networkSettingsController.overlayConfig.enabled) {
                  await _overlay.disable();
                  _networkSettingsController.overlayConfig.enabled = false;

                  // the overlay is never visible when it is disabled
                  setState(() {
                    overlayVisible = false;
                  });
                } else {
                  await _overlay.enable();
                  _networkSettingsController.overlayConfig.enabled = true;

                  if (_stateController.isCallActive) {
                    // if the call is active, the overlay should be shown
                    _overlay.show_();

                    setState(() {
                      overlayVisible = true;
                    });
                  } else {
                    // if the call is not active, the overlay should be hidden
                    setState(() {
                      overlayVisible = false;
                    });
                  }
                }

                // save the config
                _networkSettingsController.saveOverlayConfig();
              },
              width: 110,
              height: 25,
            ),
          ],
        ),
        const SizedBox(height: 20),
        const Text('Font Size', style: TextStyle(fontSize: 18)),
        Slider(
            value:
                _networkSettingsController.overlayConfig.fontHeight.toDouble(),
            onChanged: (value) {
              _overlay.setFontHeight(height: value.round());
              _networkSettingsController.overlayConfig.fontHeight =
                  value.round();
              _networkSettingsController.saveOverlayConfig();
              setState(() {});
            },
            min: 0,
            max: 70),
        const SizedBox(height: 15),
        Row(
          children: [
            Column(
              crossAxisAlignment: CrossAxisAlignment.start,
              children: [
                const Text('Background Color', style: TextStyle(fontSize: 18)),
                const SizedBox(height: 10),
                Button(
                    text: 'Change',
                    onPressed: () {
                      colorPicker(context, (Color color) {
                        _overlay.setBackgroundColor(
                            backgroundColor: color.toARGB32());
                        _networkSettingsController
                            .overlayConfig.backgroundColor = color;
                        _networkSettingsController.saveOverlayConfig();
                        setState(() {});
                      },
                          _networkSettingsController
                              .overlayConfig.backgroundColor);
                    }),
              ],
            ),
            const SizedBox(width: 40),
            Column(
              crossAxisAlignment: CrossAxisAlignment.start,
              children: [
                const Text('Primary Font Color',
                    style: TextStyle(fontSize: 18)),
                const SizedBox(height: 10),
                Button(
                    text: 'Change',
                    onPressed: () {
                      colorPicker(context, (Color color) {
                        _overlay.setFontColor(fontColor: color.toARGB32());
                        _networkSettingsController.overlayConfig.fontColor =
                            color;
                        _networkSettingsController.saveOverlayConfig();
                        setState(() {});
                      }, _networkSettingsController.overlayConfig.fontColor);
                    }),
              ],
            )
          ],
        ),
        const SizedBox(height: 35),
        if (size.$1 > 0 && size.$2 > 0)
          OverlayPositionWidget(
            realMaxX: size.$1.toDouble(),
            realMaxY: size.$2.toDouble(),
          ),
      ],
    );
  }
}
