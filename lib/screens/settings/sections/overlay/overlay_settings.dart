import 'package:flutter/material.dart' hide Overlay;
import 'package:telepathy/controllers/index.dart';
import 'package:telepathy/screens/settings/sections/overlay/color_picker_dialog.dart';
import 'package:telepathy/screens/settings/sections/overlay/overlay_position_widget.dart';
import 'package:telepathy/src/rust/overlay/overlay.dart';
import 'package:telepathy/widgets/common/index.dart';

class OverlaySettings extends StatefulWidget {
  final Overlay overlay;
  final SettingsController controller;
  final StateController stateController;

  const OverlaySettings(
      {super.key,
      required this.overlay,
      required this.controller,
      required this.stateController});

  @override
  OverlaySettingsState createState() => OverlaySettingsState();
}

class OverlaySettingsState extends State<OverlaySettings> {
  bool overlayVisible = false;

  @override
  void dispose() {
    if (!widget.stateController.isCallActive) {
      widget.overlay.hide_();
    }

    widget.controller.saveOverlayConfig();

    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    (int, int) size = widget.overlay.screenResolution();

    return Column(
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        Row(
          children: [
            Button(
              text: overlayVisible ? "Hide overlay" : "Show overlay",
              onPressed: () {
                if (widget.stateController.isCallActive ||
                    !widget.controller.overlayConfig.enabled) {
                  return;
                } else if (overlayVisible) {
                  widget.overlay.hide_();
                } else {
                  widget.overlay.show_();
                }

                setState(() {
                  overlayVisible = !overlayVisible;
                });
              },
              disabled: widget.stateController.isCallActive ||
                  !widget.controller.overlayConfig.enabled,
              width: 90,
              height: 25,
            ),
            const SizedBox(width: 20),
            Button(
              text: widget.controller.overlayConfig.enabled
                  ? "Disable overlay"
                  : "Enable overlay",
              onPressed: () async {
                if (widget.controller.overlayConfig.enabled) {
                  await widget.overlay.disable();
                  widget.controller.overlayConfig.enabled = false;

                  // the overlay is never visible when it is disabled
                  setState(() {
                    overlayVisible = false;
                  });
                } else {
                  await widget.overlay.enable();
                  widget.controller.overlayConfig.enabled = true;

                  if (widget.stateController.isCallActive) {
                    // if the call is active, the overlay should be shown
                    widget.overlay.show_();

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
                widget.controller.saveOverlayConfig();
              },
              width: 110,
              height: 25,
            ),
          ],
        ),
        const SizedBox(height: 20),
        const Text('Font Size', style: TextStyle(fontSize: 18)),
        Slider(
            value: widget.controller.overlayConfig.fontHeight.toDouble(),
            onChanged: (value) {
              widget.overlay.setFontHeight(height: value.round());
              widget.controller.overlayConfig.fontHeight = value.round();
              widget.controller.saveOverlayConfig();
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
                        widget.overlay.setBackgroundColor(
                            backgroundColor: color.toARGB32());
                        widget.controller.overlayConfig.backgroundColor = color;
                        widget.controller.saveOverlayConfig();
                        setState(() {});
                      }, widget.controller.overlayConfig.backgroundColor);
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
                        widget.overlay
                            .setFontColor(fontColor: color.toARGB32());
                        widget.controller.overlayConfig.fontColor = color;
                        widget.controller.saveOverlayConfig();
                        setState(() {});
                      }, widget.controller.overlayConfig.fontColor);
                    }),
              ],
            )
          ],
        ),
        const SizedBox(height: 35),
        if (size.$1 > 0 && size.$2 > 0)
          OverlayPositionWidget(
            overlay: widget.overlay,
            controller: widget.controller,
            realMaxX: size.$1.toDouble(),
            realMaxY: size.$2.toDouble(),
          ),
      ],
    );
  }
}
