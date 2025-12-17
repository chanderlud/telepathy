String formatTime(int milliseconds) {
  int seconds = (milliseconds / 1000).truncate();
  int minutes = (seconds / 60).truncate();
  int hours = (minutes / 60).truncate();

  String hoursStr = (hours % 60).toString().padLeft(2, '0');
  String minutesStr = (minutes % 60).toString().padLeft(2, '0');
  String secondsStr = (seconds % 60).toString().padLeft(2, '0');

  return "$hoursStr:$minutesStr:$secondsStr";
}

String formatBandwidth(int? bytes) {
  if (bytes == null) {
    return '?';
  } else if (bytes < 100000) {
    return '${roundToTotalDigits(bytes / 1000)} KB';
  } else if (bytes < 100000000) {
    return '${roundToTotalDigits(bytes / 1000000)} MB';
  } else {
    return '${roundToTotalDigits(bytes / 1000000000)} GB';
  }
}

String roundToTotalDigits(double number) {
  // get the number of digits before the decimal point
  int integerDigits = number.abs().toInt().toString().length;

  // calculate the number of fractional digits needed
  int fractionalDigits = 3 - integerDigits;
  if (fractionalDigits < 0) {
    // if the total digits is less than the integer part, we round to the integer part
    fractionalDigits = 0;
  }

  // round to the required number of fractional digits
  return number.toStringAsFixed(fractionalDigits).padRight(4, '0');
}

