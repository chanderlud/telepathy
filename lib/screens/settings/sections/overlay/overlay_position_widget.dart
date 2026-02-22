import 'package:flutter/material.dart' hide Overlay;
import 'package:provider/provider.dart';
import 'package:telepathy/controllers/index.dart';
import 'package:telepathy/src/rust/overlay/overlay.dart';

class OverlayPositionWidget extends StatefulWidget {
  final double realMaxX;
  final double realMaxY;

  const OverlayPositionWidget(
      {super.key, required this.realMaxX, required this.realMaxY});

  @override
  OverlayPositionWidgetState createState() => OverlayPositionWidgetState();
}

class OverlayPositionWidgetState extends State<OverlayPositionWidget> {
  late double _maxX;
  late double _maxY;
  late double _x;
  late double _y;
  late double _width;
  late double _height;

  bool _isDragging = false;
  bool _isResizing = false;

  @override
  void initState() {
    super.initState();

    _maxX = 650.0;
    _updatePositions();
  }

  void _updatePositions() {
    final networkSettingsController = context.read<NetworkSettingsController>();

    _maxY = _maxX / (widget.realMaxX / widget.realMaxY);

    _x = networkSettingsController.overlayConfig.x / widget.realMaxX * _maxX;
    _y = networkSettingsController.overlayConfig.y / widget.realMaxY * _maxY;
    _width =
        networkSettingsController.overlayConfig.width / widget.realMaxX * _maxX;
    _height = networkSettingsController.overlayConfig.height /
        widget.realMaxY *
        _maxY;
  }

  void _updateOverlay() {
    final overlay = context.read<Overlay>();
    final networkSettingsController = context.read<NetworkSettingsController>();

    double realX = _x / _maxX * widget.realMaxX;
    double realY = _y / _maxY * widget.realMaxY;
    double realWidth = _width / _maxX * widget.realMaxX;
    double realHeight = _height / _maxY * widget.realMaxY;

    overlay.moveOverlay(
      x: realX.round(),
      y: realY.round(),
      width: realWidth.round(),
      height: realHeight.round(),
    );

    networkSettingsController.overlayConfig.x = realX;
    networkSettingsController.overlayConfig.y = realY;
    networkSettingsController.overlayConfig.width = realWidth;
    networkSettingsController.overlayConfig.height = realHeight;
  }

  void _onDragUpdate(DragUpdateDetails details) {
    if (_isDragging) {
      setState(() {
        _x += details.delta.dx;
        _y += details.delta.dy;

        if (_x < 0) {
          _x = 0;
        } else if (_x + _width > _maxX) {
          _x = _maxX - _width;
        }

        if (_y < 0) {
          _y = 0;
        } else if (_y + _height > _maxY) {
          _y = _maxY - _height;
        }

        _updateOverlay();
      });
    }
  }

  void _onResizeUpdate(DragUpdateDetails details) {
    if (_isResizing) {
      setState(() {
        _width += details.delta.dx;
        _height += details.delta.dy;

        if (_width + _x > _maxX) {
          _width = _maxX - _x;
        } else if (_width < 10) {
          _width = 10;
        }

        if (_height + _y > _maxY) {
          _height = _maxY - _y;
        } else if (_height < 10) {
          _height = 10;
        }

        _updateOverlay();
      });
    }
  }

  void _startDragging() {
    setState(() {
      _isDragging = true;
    });
  }

  void _startResizing() {
    setState(() {
      _isResizing = true;
    });
  }

  void _stopDragging() {
    setState(() {
      _isDragging = false;
    });
  }

  void _stopResizing() {
    setState(() {
      _isResizing = false;
    });
  }

  @override
  Widget build(BuildContext context) {
    final networkSettingsController = context.read<NetworkSettingsController>();

    return LayoutBuilder(
      builder: (context, constraints) {
        _maxX = constraints.maxWidth;
        _updatePositions();

        return Container(
          decoration: BoxDecoration(
            border: Border.all(color: Colors.black, width: 2),
            color: Theme.of(context).colorScheme.surface,
          ),
          height: _maxY,
          child: Stack(
            children: [
              Positioned(
                left: _x,
                top: _y,
                child: GestureDetector(
                  onPanUpdate: _onDragUpdate,
                  onPanStart: (_) => _startDragging(),
                  onPanEnd: (_) => _stopDragging(),
                  child: Container(
                      decoration: BoxDecoration(
                        color: networkSettingsController
                            .overlayConfig.backgroundColor,
                        border: Border.all(
                            color: Theme.of(context).colorScheme.secondary,
                            width: 2),
                      ),
                      child: MouseRegion(
                        cursor: SystemMouseCursors.move,
                        child: SizedBox(
                          width: _width,
                          height: _height,
                        ),
                      )),
                ),
              ),
              Positioned(
                left: _x + _width - 10,
                top: _y + _height - 10,
                child: GestureDetector(
                  onPanUpdate: _onResizeUpdate,
                  onPanStart: (_) => _startResizing(),
                  onPanEnd: (_) => _stopResizing(),
                  child: const MouseRegion(
                    cursor: SystemMouseCursors.resizeDownRight,
                    child: SizedBox(
                      width: 20,
                      height: 20,
                    ),
                  ),
                ),
              ),
            ],
          ),
        );
      },
    );
  }
}
