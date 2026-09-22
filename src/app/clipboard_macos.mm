#import <AppKit/AppKit.h>

#include <string>

namespace niminal::app {

std::string macos_clipboard_image() {
  @autoreleasepool {
    NSPasteboard* board = [NSPasteboard generalPasteboard];
    NSData* data = [board dataForType:NSPasteboardTypePNG];
    if (data == nil) {
      NSData* tiff = [board dataForType:NSPasteboardTypeTIFF];
      if (tiff != nil) {
        NSBitmapImageRep* bitmap = [[NSBitmapImageRep alloc] initWithData:tiff];
        data = [bitmap representationUsingType:NSBitmapImageFileTypePNG properties:@{}];
      }
    }
    if (data == nil) {
      return {};
    }
    return std::string(static_cast<const char*>([data bytes]), [data length]);
  }
}

} // namespace niminal::app
