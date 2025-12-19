import 'package:flutter/material.dart';
import 'package:flutter_svg/flutter_svg.dart';
import 'package:telepathy/controllers/index.dart';
import 'package:telepathy/widgets/common/index.dart';

/// A widget which displays details about the call.
class CallDetailsWidget extends StatelessWidget {
  final StatisticsController statisticsController;
  final StateController stateController;

  const CallDetailsWidget(
      {super.key,
      required this.statisticsController,
      required this.stateController});

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
          ListenableBuilder(
              listenable: stateController,
              builder: (BuildContext context, Widget? child) {
                return Text(
                    '${stateController.activeRoom != null ? "Room" : "Call"} ${stateController.status.toLowerCase()}',
                    style: const TextStyle(fontSize: 20));
              }),
          const SizedBox(height: 8),
          Expanded(
            child: Column(
              crossAxisAlignment: CrossAxisAlignment.start,
              children: [
                Selector<StatisticsController, List<int>>(
                  listenable: statisticsController,
                  selector: (c) => c.lossWindow,
                  builder: (context, lossWindow) {
                    return GradientMiniLineChart(
                        values: lossWindow, strokeWidth: 2);
                  },
                ),
                const SizedBox(height: 6),
                const Text('Input level'),
                const SizedBox(height: 7),
                Selector<StatisticsController, double>(
                  listenable: statisticsController,
                  selector: (c) => c.inputLevel,
                  builder: (context, inputLevel) {
                    return AudioLevel(level: inputLevel, numRectangles: 20);
                  },
                ),
                const SizedBox(height: 9),
                const Text('Output level'),
                const SizedBox(height: 7),
                Selector<StatisticsController, double>(
                  listenable: statisticsController,
                  selector: (c) => c.outputLevel,
                  builder: (context, outputLevel) {
                    return AudioLevel(level: outputLevel, numRectangles: 20);
                  },
                ),
                const SizedBox(height: 12),
                Row(
                  mainAxisSize: MainAxisSize.min,
                  children: [
                    Selector<StatisticsController, int>(
                      listenable: statisticsController,
                      selector: (c) => c.latency,
                      builder: (context, latency) {
                        Color color = getColor(latency / 200);
                        return SvgPicture.asset('assets/icons/Latency.svg',
                            colorFilter:
                                ColorFilter.mode(color, BlendMode.srcIn),
                            semanticsLabel: 'Latency icon');
                      },
                    ),
                    const SizedBox(width: 7),
                    Selector<StatisticsController, int>(
                      listenable: statisticsController,
                      selector: (c) => c.latency,
                      builder: (context, latency) {
                        return Text('$latency ms',
                            style: const TextStyle(height: 0));
                      },
                    ),
                    const Spacer(),
                    SvgPicture.asset('assets/icons/Upload.svg',
                        semanticsLabel: 'Upload icon'),
                    const SizedBox(width: 4),
                    Selector<StatisticsController, String>(
                      listenable: statisticsController,
                      selector: (c) => c.upload,
                      builder: (context, upload) {
                        return Text(upload, style: const TextStyle(height: 0));
                      },
                    ),
                    const Spacer(),
                    SvgPicture.asset('assets/icons/Download.svg',
                        semanticsLabel: 'Download icon'),
                    const SizedBox(width: 4),
                    Selector<StatisticsController, String>(
                      listenable: statisticsController,
                      selector: (c) => c.download,
                      builder: (context, download) {
                        return Text(download, style: const TextStyle(height: 0));
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
