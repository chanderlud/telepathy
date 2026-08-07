import 'dart:io';

import 'package:flutter_test/flutter_test.dart';

void main() {
  test('web loading screen remains branded until Flutter first frame', () {
    final indexHtml = File('web/index.html').readAsStringSync();
    const firstFrameListener = "window.addEventListener('flutter-first-frame'";
    const bootstrapScript =
        '<script src="flutter_bootstrap.js" async></script>';

    expect(indexHtml, contains('<title>Telepathy</title>'));
    expect(indexHtml, contains(bootstrapScript));
    expect(indexHtml, contains('id="telepathy-loader"'));
    expect(indexHtml, contains('role="status"'));
    expect(indexHtml, contains('aria-live="polite"'));
    expect(indexHtml, contains('icons/Icon-512.png'));
    expect(indexHtml, contains('#222425'));
    expect(indexHtml, contains('#5538e5'));
    expect(indexHtml, contains('transition: opacity'));
    expect(indexHtml, contains("loader.classList.add('is-hidden')"));
    expect(indexHtml, contains('loader.remove()'));

    final firstFrameListenerIndex = indexHtml.indexOf(firstFrameListener);
    final bootstrapScriptIndex = indexHtml.indexOf(bootstrapScript);
    expect(firstFrameListenerIndex, greaterThanOrEqualTo(0));
    expect(bootstrapScriptIndex, greaterThan(firstFrameListenerIndex));
  });
}
