import 'package:flutter/material.dart';
import 'package:telepathy/console.dart';

class LogsSettings extends StatefulWidget {
  final TextEditingController searchController;

  const LogsSettings({super.key, required this.searchController});

  @override
  State<LogsSettings> createState() => _LogsSettingsState();
}

class _LogsSettingsState extends State<LogsSettings> {
  @override
  Widget build(BuildContext context) {
    final filter = widget.searchController.text.isEmpty
        ? null
        : widget.searchController.text;
    final logs = console.getLogs(filter);

    return Column(
      children: [
        TextField(
          controller: widget.searchController,
          decoration: const InputDecoration(labelText: 'Search'),
          onChanged: (_) => setState(() {}),
        ),
        const SizedBox(height: 20),
        ListView.builder(
          itemCount: logs.length,
          shrinkWrap: true,
          itemBuilder: (context, index) {
            final log = logs[index];
            return SelectableText(
              '${log.time} - ${log.type}: ${log.message}',
            );
          },
        ),
      ],
    );
  }
}
