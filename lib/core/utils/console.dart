Console console = Console();

class DebugConsole {
  static void log(String message) {
    console.push(message, 'log');
  }

  static void error(String message) {
    console.push(message, 'error');
  }

  static void warn(String message) {
    console.push(message, 'warning');
  }

  static void info(String message) {
    console.push(message, 'info');
  }

  static void debug(String message) {
    console.push(message, 'debug');
  }
}

class Console {
  List<Log> logs = [];

  void push(String message, String type) {
    logs.add(Log(message, type, DateTime.now()));
  }

  List<Log> getLogs(String? filter) {
    if (filter == null) {
      return logs;
    }

    String lowerFilter = filter.toLowerCase();
    return logs
        .where((log) => log.message.toLowerCase().contains(lowerFilter))
        .toList();
  }
}

class Log {
  String message;
  String type;
  DateTime time;

  Log(this.message, this.type, this.time);
}
