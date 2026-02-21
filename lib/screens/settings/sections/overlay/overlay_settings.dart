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

  @override
  void dispose() {
    final overlay = context.read<Overlay>();
    final stateController = context.read<StateController>();
    final networkSettingsController = context.read<NetworkSettingsController>();

    if (!stateController.isCallActive) {
      overlay.hide_();
    }

    networkSettingsController.saveOverlayConfig();

    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    final overlay = context.read<Overlay>();
    final stateController = context.read<StateController>();
    final networkSettingsController = context.read<NetworkSettingsController>();

    (int, int) size = overlay.screenResolution();

    return Column(
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        Row(
          children: [
            Button(
              text: overlayVisible ? 'Hide overlay' : 'Show overlay',
              onPressed: () {
                if (stateController.isCallActive ||
                    !networkSettingsController.overlayConfig.enabled) {
                  return;
                } else if (overlayVisible) {
                  overlay.hide_();
                } else {
                  overlay.show_();
                }

                setState(() {
                  overlayVisible = !overlayVisible;
                });
              },
              disabled: stateController.isCallActive ||
                  !networkSettingsController.overlayConfig.enabled,
              width: 90,
              height: 25,
            ),
            const SizedBox(width: 20),
            Button(
              text: networkSettingsController.overlayConfig.enabled
                  ? 'Disable overlay'
                  : 'Enable overlay',
              onPressed: () async {
                if (networkSettingsController.overlayConfig.enabled) {
                  await overlay.disable();
                  networkSettingsController.overlayConfig.enabled = false;

                  // the overlay is never visible when it is disabled
                  setState(() {
                    overlayVisible = false;
                  });
                } else {
                  await overlay.enable();
                  networkSettingsController.overlayConfig.enabled = true;

                  if (stateController.isCallActive) {
                    // if the call is active, the overlay should be shown
                    overlay.show_();

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
                networkSettingsController.saveOverlayConfig();
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
                networkSettingsController.overlayConfig.fontHeight.toDouble(),
            onChanged: (value) {
              overlay.setFontHeight(height: value.round());
              networkSettingsController.overlayConfig.fontHeight =
                  value.round();
              networkSettingsController.saveOverlayConfig();
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
                        overlay.setBackgroundColor(
                            backgroundColor: color.toARGB32());
                        networkSettingsController
                            .overlayConfig.backgroundColor = color;
                        networkSettingsController.saveOverlayConfig();
                        setState(() {});
                      },
                          networkSettingsController
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
                        overlay.setFontColor(fontColor: color.toARGB32());
                        networkSettingsController.overlayConfig.fontColor =
                            color;
                        networkSettingsController.saveOverlayConfig();
                        setState(() {});
                      }, networkSettingsController.overlayConfig.fontColor);
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
