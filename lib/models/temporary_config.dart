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

