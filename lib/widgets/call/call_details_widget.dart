import 'package:flutter/material.dart';
import 'package:flutter_svg/flutter_svg.dart';
import 'package:provider/provider.dart';
import 'package:telepathy/controllers/index.dart';
import 'package:telepathy/widgets/common/index.dart';

/// A widget which displays details about the call.
class CallDetailsWidget extends StatelessWidget {
  const CallDetailsWidget({super.key});

  @override
  Widget build(BuildContext context) {
    return Container(
      padding: const EdgeInsets.symmetric(vertical: 15.0, horizontal: 20.0),
      decoration: BoxDecoration(
        color: Theme.of(context).colorScheme.secondaryContainer,
        borderRadius: BorderRadius.circular(10.0),
      ),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Consumer<StateController>(builder:
              (BuildContext context, StateController stateController, _) {
            return Text(
                '${stateController.activeRoom != null ? "Room" : "Call"} ${stateController.status.toLowerCase()}',
                style: const TextStyle(fontSize: 20));
          }),
          const SizedBox(height: 8),
          Expanded(
            child: Column(
              crossAxisAlignment: CrossAxisAlignment.start,
              children: [
                RepaintBoundary(
                  child: Selector<StatisticsController, int>(
                    selector: (context, c) => c.lossWindowVersion,
                    builder: (context, version, child) {
                      final controller = context.read<StatisticsController>();
                      return GradientMiniLineChart(
                        values: controller.lossWindow,
                        version: version,
                        maxValue: controller.lossWindowMax,
                        strokeWidth: 2,
                      );
                    },
                  ),
                ),
                const SizedBox(height: 6),
                const Text('Input level'),
                const SizedBox(height: 7),
                RepaintBoundary(
                  child: Selector<StatisticsController, double>(
                    selector: (context, c) => c.inputLevel,
                    builder: (context, inputLevel, child) {
                      return AudioLevel(level: inputLevel, numRectangles: 20);
                    },
                  ),
                ),
                const SizedBox(height: 9),
                const Text('Output level'),
                const SizedBox(height: 7),
                RepaintBoundary(
                  child: Selector<StatisticsController, double>(
                    selector: (context, c) => c.outputLevel,
                    builder: (context, outputLevel, child) {
                      return AudioLevel(level: outputLevel, numRectangles: 20);
                    },
                  ),
                ),
                const SizedBox(height: 12),
                Row(
                  mainAxisSize: MainAxisSize.min,
                  children: [
                    Selector<StatisticsController, int>(
                      selector: (context, c) => c.latency,
                      builder: (context, latency, child) {
                        Color color = getColor(latency / 200);
                        return Row(
                          mainAxisSize: MainAxisSize.min,
                          children: [
                            SvgPicture.asset('assets/icons/Latency.svg',
                                colorFilter:
                                    ColorFilter.mode(color, BlendMode.srcIn),
                                semanticsLabel: 'Latency icon'),
                            const SizedBox(width: 7),
                            Text('$latency ms',
                                style: const TextStyle(height: 0)),
                          ],
                        );
                      },
                    ),
                    const Spacer(),
                    SvgPicture.asset('assets/icons/Upload.svg',
                        semanticsLabel: 'Upload icon'),
                    const SizedBox(width: 4),
                    Selector<StatisticsController, String>(
                      selector: (context, c) => c.upload,
                      builder: (context, upload, child) {
                        return Text(upload, style: const TextStyle(height: 0));
                      },
                    ),
                    const Spacer(),
                    SvgPicture.asset('assets/icons/Download.svg',
                        semanticsLabel: 'Download icon'),
                    const SizedBox(width: 4),
                    Selector<StatisticsController, String>(
                      selector: (context, c) => c.download,
                      builder: (context, download, child) {
                        return Text(download,
                            style: const TextStyle(height: 0));
                      },
                    ),
                  ],
                ),
              ],
            ),
          ),
        ],
      ),
    );
  }
}
