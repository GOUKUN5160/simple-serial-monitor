#import <Cocoa/Cocoa.h>
#import <IOKit/IOKitLib.h>
#import <IOKit/IOMessage.h>
#import <IOKit/serial/IOSerialKeys.h>
#import "bridge.h"

// ============================================================================
// Global State & Helpers
// ============================================================================

static BOOL gIsJa = NO;
static NSString *gThemeMode = @"system";   // system | light | dark
static NSString *gNotifyMode = @"in_app";  // off | in_app | windows | both
static int gConfiguredApiPort = 27182;
static int gActualApiPort = -1;
static BOOL gShuttingDown = NO;

static inline NSString *TR(NSString *en, NSString *ja) {
    return gIsJa ? ja : en;
}

static id SsmParseJsonC(char *cstr) {
    if (!cstr) return nil;
    NSData *data = [NSData dataWithBytes:cstr length:strlen(cstr)];
    ssm_free_str(cstr);
    if (!data.length) return nil;
    return [NSJSONSerialization JSONObjectWithData:data options:0 error:nil];
}

static NSString *SsmToJsonString(id obj) {
    if (!obj) return @"{}";
    NSData *data = [NSJSONSerialization dataWithJSONObject:obj options:0 error:nil];
    if (!data) return @"{}";
    return [[NSString alloc] initWithData:data encoding:NSUTF8StringEncoding] ?: @"{}";
}

static BOOL SsmIsDarkActive(void) {
    if ([gThemeMode isEqualToString:@"dark"]) return YES;
    if ([gThemeMode isEqualToString:@"light"]) return NO;
    NSAppearance *app = [NSApp effectiveAppearance];
    NSAppearanceName best = [app bestMatchFromAppearancesWithNames:@[
        NSAppearanceNameAqua,
        NSAppearanceNameDarkAqua
    ]];
    return [best isEqualToString:NSAppearanceNameDarkAqua];
}

static NSColor *SsmRGB(CGFloat r, CGFloat g, CGFloat b) {
    return [NSColor colorWithSRGBRed:r / 255.0 green:g / 255.0 blue:b / 255.0 alpha:1.0];
}

static NSString *SsmShortPortLabel(NSString *port, NSString *hw) {
    NSString *shortName = port;
    if ([shortName hasPrefix:@"/dev/"]) {
        shortName = [shortName substringFromIndex:5];
    }
    if (hw.length > 0 && ![hw isEqualToString:shortName] && ![hw isEqualToString:port]) {
        return [NSString stringWithFormat:@"%@ - %@", shortName, hw];
    }
    return shortName;
}

static NSString *SsmEolLabel(NSString *code) {
    if ([code isEqualToString:@"crlf"]) return @"CRLF";
    if ([code isEqualToString:@"lf"]) return @"LF";
    if ([code isEqualToString:@"cr"]) return @"CR";
    if ([code isEqualToString:@"none"]) return @"None";
    return @"Auto";
}

@class SsmAppDelegate;
@class SsmMonitorWindowController;
@class SsmPaneView;

static SsmAppDelegate *gAppDelegate = nil;

// ============================================================================
// Send Input Field with History Navigation (Up/Down) & Enter
// ============================================================================

@protocol SsmSendFieldDelegate <NSObject>
- (void)sendFieldDidSubmit:(NSString *)text;
- (void)sendFieldHistoryPrev;
- (void)sendFieldHistoryNext;
@end

@interface SsmSendField : NSTextField <NSTextFieldDelegate>
@property (nonatomic, weak) id<SsmSendFieldDelegate> sendDelegate;
@end

@implementation SsmSendField

- (instancetype)initWithFrame:(NSRect)frameRect {
    self = [super initWithFrame:frameRect];
    if (self) {
        self.delegate = self;
        self.bezelStyle = NSTextFieldSquareBezel;
        self.font = [NSFont monospacedSystemFontOfSize:12 weight:NSFontWeightRegular];
        self.usesSingleLineMode = YES;
        self.cell.scrollable = YES;
    }
    return self;
}

- (BOOL)control:(NSControl *)control textView:(NSTextView *)textView doCommandBySelector:(SEL)commandSelector {
    if (commandSelector == @selector(insertNewline:)) {
        [self.sendDelegate sendFieldDidSubmit:self.stringValue ?: @""];
        return YES;
    }
    if (commandSelector == @selector(moveUp:)) {
        [self.sendDelegate sendFieldHistoryPrev];
        return YES;
    }
    if (commandSelector == @selector(moveDown:)) {
        [self.sendDelegate sendFieldHistoryNext];
        return YES;
    }
    return NO;
}

@end

// ============================================================================
// High-Performance Virtual Log View (NSView inside NSScrollView)
// ============================================================================

@interface SsmVisualLine : NSObject
@property (nonatomic, assign) NSInteger entryIndex;
@property (nonatomic, copy) NSString *text;
@property (nonatomic, copy) NSString *timestamp;
@property (nonatomic, copy) NSString *kind;
@property (nonatomic, assign) BOOL isSys;
@property (nonatomic, assign) BOOL isFirstLineOfEntry;
@end

@implementation SsmVisualLine
@end

@class SsmLogView;

@interface SsmLogContentView : NSView
@property (nonatomic, weak) SsmLogView *owner;
@property (nonatomic, strong) NSMutableArray<SsmVisualLine *> *visualLines;
@property (nonatomic, assign) CGFloat lineHeight;
@property (nonatomic, assign) CGFloat charWidth;
@property (nonatomic, assign) NSInteger selAnchor;
@property (nonatomic, assign) NSInteger selActive;
@property (nonatomic, assign) BOOL dragging;
@end

@interface SsmLogView : NSView
@property (nonatomic, strong) NSScrollView *scrollView;
@property (nonatomic, strong) SsmLogContentView *contentView;
@property (nonatomic, strong) NSMutableArray<NSDictionary *> *entries;
@property (nonatomic, copy) NSString *displayMode; // text | hex | bin
@property (nonatomic, assign) BOOL autoScroll;
@property (nonatomic, assign) BOOL showTimestamps;
@property (nonatomic, assign) BOOL wordWrap;
@property (nonatomic, assign) NSInteger zoomPercent;
@property (nonatomic, assign) BOOL programmaticScroll;
@property (nonatomic, copy) void (^onAutoScrollChanged)(BOOL autoScroll);
@property (nonatomic, copy) void (^onZoomChanged)(NSInteger zoomPercent);

- (void)appendEntries:(NSArray<NSDictionary *> *)newEntries;
- (void)clearAll;
- (void)rebuildVisualLines;
- (void)adjustZoom:(NSInteger)delta;
- (void)resetZoom;
- (void)refreshAppearance;
- (void)scrollToBottom;
@end

@implementation SsmLogContentView

- (BOOL)isFlipped {
    return YES;
}

- (BOOL)acceptsFirstResponder {
    return YES;
}

- (BOOL)isOpaque {
    return YES;
}

- (NSFont *)currentMonoFont {
    CGFloat sz = MAX(7.0, round(12.0 * (self.owner.zoomPercent / 100.0)));
    return [NSFont monospacedSystemFontOfSize:sz weight:NSFontWeightRegular];
}

- (NSFont *)currentSysFont {
    CGFloat sz = MAX(7.0, round(11.5 * (self.owner.zoomPercent / 100.0)));
    return [NSFont systemFontOfSize:sz weight:NSFontWeightSemibold];
}

- (void)drawRect:(NSRect)dirtyRect {
    BOOL dark = SsmIsDarkActive();
    NSColor *bg = dark ? SsmRGB(18, 21, 26) : SsmRGB(252, 253, 255);
    [bg setFill];
    NSRectFill(dirtyRect);

    NSInteger total = (NSInteger)self.visualLines.count;
    if (total == 0 || self.lineHeight <= 0) return;

    NSInteger firstRow = MAX(0, (NSInteger)floor(NSMinY(dirtyRect) / self.lineHeight) - 1);
    NSInteger lastRow = MIN(total - 1, (NSInteger)ceil(NSMaxY(dirtyRect) / self.lineHeight) + 1);

    NSFont *monoFont = [self currentMonoFont];
    NSFont *sysFont = [self currentSysFont];

    NSColor *rxColor = dark ? SsmRGB(228, 234, 242) : SsmRGB(24, 28, 34);
    NSColor *txColor = dark ? SsmRGB(79, 195, 247) : SsmRGB(2, 119, 189);
    NSColor *tsColor = dark ? SsmRGB(122, 134, 152) : SsmRGB(110, 120, 134);
    NSColor *selBg = dark ? SsmRGB(38, 79, 120) : SsmRGB(186, 216, 248);

    NSInteger selMin = -1, selMax = -1;
    if (self.selAnchor >= 0 && self.selActive >= 0) {
        selMin = MIN(self.selAnchor, self.selActive);
        selMax = MAX(self.selAnchor, self.selActive);
    }

    CGFloat fullW = MAX(self.bounds.size.width, self.enclosingScrollView.contentSize.width);

    for (NSInteger row = firstRow; row <= lastRow; row++) {
        SsmVisualLine *vl = self.visualLines[row];
        NSRect rowRect = NSMakeRect(0, row * self.lineHeight, fullW, self.lineHeight);
        BOOL selected = (selMin >= 0 && row >= selMin && row <= selMax);

        if (vl.isSys) {
            NSColor *bannerBg = nil;
            NSColor *accent = nil;
            NSColor *bannerFg = nil;
            NSString *icon = @"•";
            if ([vl.kind isEqualToString:@"sys_connect"]) {
                bannerBg = dark ? SsmRGB(20, 48, 30) : SsmRGB(224, 245, 232);
                accent = dark ? SsmRGB(56, 182, 92) : SsmRGB(27, 118, 56);
                bannerFg = dark ? SsmRGB(154, 235, 178) : SsmRGB(20, 92, 44);
                icon = @"●";
            } else if ([vl.kind isEqualToString:@"sys_disconnect"]) {
                bannerBg = dark ? SsmRGB(58, 38, 16) : SsmRGB(254, 240, 218);
                accent = dark ? SsmRGB(224, 146, 36) : SsmRGB(176, 104, 0);
                bannerFg = dark ? SsmRGB(252, 206, 132) : SsmRGB(138, 78, 0);
                icon = @"○";
            } else if ([vl.kind isEqualToString:@"sys_error"]) {
                bannerBg = dark ? SsmRGB(62, 22, 24) : SsmRGB(253, 226, 226);
                accent = dark ? SsmRGB(229, 72, 77) : SsmRGB(198, 40, 40);
                bannerFg = dark ? SsmRGB(255, 168, 170) : SsmRGB(160, 24, 24);
                icon = @"▲";
            } else {
                bannerBg = dark ? SsmRGB(44, 26, 62) : SsmRGB(238, 228, 252);
                accent = dark ? SsmRGB(162, 96, 232) : SsmRGB(112, 54, 184);
                bannerFg = dark ? SsmRGB(214, 178, 255) : SsmRGB(86, 36, 150);
                icon = @"◆";
            }
            [(selected ? selBg : bannerBg) setFill];
            NSRectFill(NSInsetRect(rowRect, 4, 1));
            [accent setFill];
            NSRectFill(NSMakeRect(4, rowRect.origin.y + 1, 3, rowRect.size.height - 2));

            NSString *label = [NSString stringWithFormat:@"— %@ %@ %@ —", icon, vl.timestamp ?: @"", vl.text ?: @""];
            NSMutableParagraphStyle *ps = [[NSMutableParagraphStyle alloc] init];
            ps.alignment = NSTextAlignmentCenter;
            ps.lineBreakMode = NSLineBreakByTruncatingTail;
            NSDictionary *attrs = @{
                NSFontAttributeName: sysFont,
                NSForegroundColorAttributeName: bannerFg,
                NSParagraphStyleAttributeName: ps
            };
            CGFloat textH = ceil(sysFont.ascender - sysFont.descender);
            CGFloat ty = rowRect.origin.y + floor((self.lineHeight - textH) * 0.5);
            [label drawInRect:NSMakeRect(10, ty, MAX(20, fullW - 20), self.lineHeight) withAttributes:attrs];
        } else {
            if (selected) {
                [selBg setFill];
                NSRectFill(rowRect);
            }
            CGFloat x = 8.0;
            CGFloat textH = ceil(monoFont.ascender - monoFont.descender);
            CGFloat ty = rowRect.origin.y + floor((self.lineHeight - textH) * 0.5);

            if (self.owner.showTimestamps && vl.timestamp.length > 0) {
                NSDictionary *tsAttrs = @{
                    NSFontAttributeName: monoFont,
                    NSForegroundColorAttributeName: tsColor
                };
                if (vl.isFirstLineOfEntry) {
                    [vl.timestamp drawAtPoint:NSMakePoint(x, ty) withAttributes:tsAttrs];
                }
                x += ceil(self.charWidth * (vl.timestamp.length + 2));
            }

            BOOL isTx = [vl.kind isEqualToString:@"tx"];
            NSString *lineText = isTx && vl.isFirstLineOfEntry
                ? [NSString stringWithFormat:@">> %@", vl.text ?: @""]
                : (vl.text ?: @"");
            NSDictionary *attrs = @{
                NSFontAttributeName: monoFont,
                NSForegroundColorAttributeName: isTx ? txColor : rxColor
            };
            [lineText drawAtPoint:NSMakePoint(x, ty) withAttributes:attrs];
        }
    }
}

- (NSInteger)rowForPoint:(NSPoint)pt {
    if (self.visualLines.count == 0 || self.lineHeight <= 0) return -1;
    NSInteger row = (NSInteger)floor(pt.y / self.lineHeight);
    if (row < 0) row = 0;
    if (row >= (NSInteger)self.visualLines.count) row = (NSInteger)self.visualLines.count - 1;
    return row;
}

- (void)mouseDown:(NSEvent *)event {
    [self.window makeFirstResponder:self];
    NSPoint pt = [self convertPoint:event.locationInWindow fromView:nil];
    NSInteger row = [self rowForPoint:pt];
    if (row >= 0) {
        if ((event.modifierFlags & NSEventModifierFlagShift) && self.selAnchor >= 0) {
            self.selActive = row;
        } else {
            self.selAnchor = row;
            self.selActive = row;
        }
        self.dragging = YES;
        [self setNeedsDisplay:YES];
    }
}

- (void)mouseDragged:(NSEvent *)event {
    if (!self.dragging) return;
    NSPoint pt = [self convertPoint:event.locationInWindow fromView:nil];
    NSInteger row = [self rowForPoint:pt];
    if (row >= 0 && row != self.selActive) {
        self.selActive = row;
        [self autoscroll:event];
        [self setNeedsDisplay:YES];
    }
}

- (void)mouseUp:(NSEvent *)event {
    self.dragging = NO;
}

- (void)copy:(id)sender {
    if (self.visualLines.count == 0) return;
    NSInteger selMin = -1, selMax = -1;
    if (self.selAnchor >= 0 && self.selActive >= 0) {
        selMin = MIN(self.selAnchor, self.selActive);
        selMax = MAX(self.selAnchor, self.selActive);
    } else {
        return;
    }
    NSMutableArray<NSString *> *parts = [NSMutableArray array];
    for (NSInteger i = selMin; i <= selMax && i < (NSInteger)self.visualLines.count; i++) {
        SsmVisualLine *vl = self.visualLines[i];
        if (self.owner.showTimestamps && vl.timestamp.length > 0 && vl.isFirstLineOfEntry) {
            [parts addObject:[NSString stringWithFormat:@"[%@] %@", vl.timestamp, vl.text ?: @""]];
        } else {
            [parts addObject:vl.text ?: @""];
        }
    }
    NSString *joined = [parts componentsJoinedByString:@"\n"];
    NSPasteboard *pb = [NSPasteboard generalPasteboard];
    [pb clearContents];
    [pb setString:joined forType:NSPasteboardTypeString];
}

- (void)selectAll:(id)sender {
    if (self.visualLines.count == 0) return;
    self.selAnchor = 0;
    self.selActive = (NSInteger)self.visualLines.count - 1;
    [self setNeedsDisplay:YES];
}

- (void)scrollWheel:(NSEvent *)event {
    NSEventModifierFlags mods = event.modifierFlags;
    if ((mods & NSEventModifierFlagCommand) || (mods & NSEventModifierFlagControl)) {
        if (event.scrollingDeltaY > 0.1) {
            [self.owner adjustZoom:10];
        } else if (event.scrollingDeltaY < -0.1) {
            [self.owner adjustZoom:-10];
        }
        return;
    }
    [super scrollWheel:event];
}

- (void)keyDown:(NSEvent *)event {
    NSEventModifierFlags mods = event.modifierFlags & NSEventModifierFlagDeviceIndependentFlagsMask;
    NSString *chars = event.charactersIgnoringModifiers;
    if (mods == NSEventModifierFlagCommand) {
        if ([chars isEqualToString:@"c"]) { [self copy:nil]; return; }
        if ([chars isEqualToString:@"a"]) { [self selectAll:nil]; return; }
        if ([chars isEqualToString:@"+"] || [chars isEqualToString:@"="]) { [self.owner adjustZoom:10]; return; }
        if ([chars isEqualToString:@"-"]) { [self.owner adjustZoom:-10]; return; }
        if ([chars isEqualToString:@"0"]) { [self.owner resetZoom]; return; }
    }
    [super keyDown:event];
}

@end

@implementation SsmLogView

- (instancetype)initWithFrame:(NSRect)frameRect {
    self = [super initWithFrame:frameRect];
    if (self) {
        self.entries = [NSMutableArray array];
        self.displayMode = @"text";
        self.autoScroll = YES;
        self.showTimestamps = NO;
        self.wordWrap = NO;
        self.zoomPercent = 100;

        self.scrollView = [[NSScrollView alloc] initWithFrame:self.bounds];
        self.scrollView.autoresizingMask = NSViewWidthSizable | NSViewHeightSizable;
        self.scrollView.hasVerticalScroller = YES;
        self.scrollView.hasHorizontalScroller = YES;
        self.scrollView.borderType = NSBezelBorder;
        self.scrollView.drawsBackground = YES;

        self.contentView = [[SsmLogContentView alloc] initWithFrame:NSMakeRect(0, 0, frameRect.size.width, 20)];
        self.contentView.owner = self;
        self.contentView.visualLines = [NSMutableArray array];
        self.contentView.selAnchor = -1;
        self.contentView.selActive = -1;

        self.scrollView.documentView = self.contentView;
        [self addSubview:self.scrollView];

        self.scrollView.contentView.postsBoundsChangedNotifications = YES;
        [[NSNotificationCenter defaultCenter] addObserver:self
                                                 selector:@selector(clipViewBoundsChanged:)
                                                     name:NSViewBoundsDidChangeNotification
                                                   object:self.scrollView.contentView];
        [self refreshAppearance];
    }
    return self;
}

- (void)dealloc {
    [[NSNotificationCenter defaultCenter] removeObserver:self];
}

- (void)setFrameSize:(NSSize)newSize {
    BOOL widthChanged = fabs(newSize.width - self.frame.size.width) > 1.0;
    [super setFrameSize:newSize];
    if (widthChanged && self.wordWrap) {
        [self rebuildVisualLines];
    } else {
        [self updateDocumentFrame];
    }
}

- (void)refreshAppearance {
    BOOL dark = SsmIsDarkActive();
    self.scrollView.backgroundColor = dark ? SsmRGB(18, 21, 26) : SsmRGB(252, 253, 255);
    [self.contentView setNeedsDisplay:YES];
}

- (void)clipViewBoundsChanged:(NSNotification *)note {
    if (self.programmaticScroll) return;
    NSRect vis = self.scrollView.documentVisibleRect;
    CGFloat docH = self.contentView.frame.size.height;
    BOOL atBottom = (NSMaxY(vis) >= docH - self.contentView.lineHeight * 1.5) || (docH <= vis.size.height + 2.0);
    if (!atBottom && self.autoScroll) {
        self.autoScroll = NO;
        if (self.onAutoScrollChanged) self.onAutoScrollChanged(NO);
    } else if (atBottom && !self.autoScroll) {
        self.autoScroll = YES;
        if (self.onAutoScrollChanged) self.onAutoScrollChanged(YES);
    }
}

- (void)computeMetrics {
    NSFont *font = [self.contentView currentMonoFont];
    CGFloat h = ceil(font.ascender - font.descender + font.leading) + 4.0;
    self.contentView.lineHeight = MAX(14.0, h);
    NSDictionary *attrs = @{ NSFontAttributeName: font };
    CGFloat w = [@"MMMMMMMMMM" sizeWithAttributes:attrs].width / 10.0;
    self.contentView.charWidth = MAX(5.0, w);
}

- (NSString *)textForEntry:(NSDictionary *)entry {
    if ([entry[@"is_sys"] boolValue]) {
        return entry[@"text"] ?: @"";
    }
    if ([self.displayMode isEqualToString:@"hex"]) {
        return entry[@"hex"] ?: entry[@"text"] ?: @"";
    }
    if ([self.displayMode isEqualToString:@"bin"]) {
        return entry[@"bin"] ?: entry[@"text"] ?: @"";
    }
    return entry[@"text"] ?: @"";
}

- (void)appendVisualLinesForEntry:(NSDictionary *)entry index:(NSInteger)idx maxCols:(NSInteger)maxCols {
    BOOL isSys = [entry[@"is_sys"] boolValue];
    NSString *kind = entry[@"kind"] ?: @"rx";
    NSString *ts = isSys ? (entry[@"datetime"] ?: @"") : (entry[@"time"] ?: @"");
    if (isSys && ts.length > 19) {
        ts = [ts substringToIndex:19];
    }
    NSString *rawText = [self textForEntry:entry];

    if (isSys || !self.wordWrap || maxCols < 12 || (NSInteger)rawText.length <= maxCols) {
        SsmVisualLine *vl = [[SsmVisualLine alloc] init];
        vl.entryIndex = idx;
        vl.text = rawText;
        vl.timestamp = ts;
        vl.kind = kind;
        vl.isSys = isSys;
        vl.isFirstLineOfEntry = YES;
        [self.contentView.visualLines addObject:vl];
        return;
    }

    NSInteger pos = 0;
    NSInteger len = (NSInteger)rawText.length;
    BOOL first = YES;
    while (pos < len) {
        NSInteger chunkLen = MIN(maxCols, len - pos);
        SsmVisualLine *vl = [[SsmVisualLine alloc] init];
        vl.entryIndex = idx;
        vl.text = [rawText substringWithRange:NSMakeRange(pos, chunkLen)];
        vl.timestamp = ts;
        vl.kind = kind;
        vl.isSys = NO;
        vl.isFirstLineOfEntry = first;
        [self.contentView.visualLines addObject:vl];
        pos += chunkLen;
        first = NO;
    }
}

- (NSInteger)currentWrapCols {
    CGFloat availW = self.scrollView.contentSize.width - 20.0;
    if (self.showTimestamps) {
        availW -= self.contentView.charWidth * 14.0;
    }
    return MAX(16, (NSInteger)floor(availW / MAX(5.0, self.contentView.charWidth)));
}

- (void)updateDocumentFrame {
    CGFloat visW = MAX(100.0, self.scrollView.contentSize.width);
    CGFloat visH = MAX(40.0, self.scrollView.contentSize.height);
    CGFloat totalH = MAX(visH, self.contentView.visualLines.count * self.contentView.lineHeight + 6.0);

    CGFloat maxW = visW;
    if (!self.wordWrap) {
        NSInteger maxChars = 0;
        for (SsmVisualLine *vl in self.contentView.visualLines) {
            if (!vl.isSys) {
                NSInteger c = (NSInteger)vl.text.length + (self.showTimestamps ? 16 : 4);
                if (c > maxChars) maxChars = c;
            }
        }
        CGFloat neededW = maxChars * self.contentView.charWidth + 24.0;
        if (neededW > maxW) maxW = neededW;
    }
    self.scrollView.hasHorizontalScroller = !self.wordWrap;
    [self.contentView setFrameSize:NSMakeSize(maxW, totalH)];
    [self.contentView setNeedsDisplay:YES];
}

- (void)rebuildVisualLines {
    [self computeMetrics];
    [self.contentView.visualLines removeAllObjects];
    NSInteger maxCols = [self currentWrapCols];
    NSInteger count = (NSInteger)self.entries.count;
    for (NSInteger i = 0; i < count; i++) {
        [self appendVisualLinesForEntry:self.entries[i] index:i maxCols:maxCols];
    }
    self.contentView.selAnchor = -1;
    self.contentView.selActive = -1;
    [self updateDocumentFrame];
    if (self.autoScroll) {
        [self scrollToBottom];
    }
}

- (void)appendEntries:(NSArray<NSDictionary *> *)newEntries {
    if (newEntries.count == 0) return;
    if (self.contentView.lineHeight <= 0) {
        [self computeMetrics];
    }
    NSInteger maxCols = [self currentWrapCols];
    BOOL trimmed = NO;
    for (NSDictionary *e in newEntries) {
        [self.entries addObject:e];
        [self appendVisualLinesForEntry:e index:(NSInteger)self.entries.count - 1 maxCols:maxCols];
    }
    if (self.entries.count > 8000) {
        NSInteger excess = (NSInteger)self.entries.count - 8000;
        [self.entries removeObjectsInRange:NSMakeRange(0, excess)];
        trimmed = YES;
    }
    if (trimmed) {
        [self rebuildVisualLines];
    } else {
        [self updateDocumentFrame];
        if (self.autoScroll) {
            [self scrollToBottom];
        }
    }
}

- (void)clearAll {
    [self.entries removeAllObjects];
    [self.contentView.visualLines removeAllObjects];
    self.contentView.selAnchor = -1;
    self.contentView.selActive = -1;
    [self updateDocumentFrame];
}

- (void)scrollToBottom {
    self.programmaticScroll = YES;
    CGFloat maxY = MAX(0.0, self.contentView.frame.size.height - self.scrollView.contentSize.height);
    [self.scrollView.contentView scrollToPoint:NSMakePoint(self.scrollView.documentVisibleRect.origin.x, maxY)];
    [self.scrollView reflectScrolledClipView:self.scrollView.contentView];
    self.programmaticScroll = NO;
}

- (void)adjustZoom:(NSInteger)delta {
    NSInteger next = MIN(300, MAX(50, self.zoomPercent + delta));
    if (next != self.zoomPercent) {
        self.zoomPercent = next;
        [self rebuildVisualLines];
        if (self.onZoomChanged) self.onZoomChanged(next);
    }
}

- (void)resetZoom {
    if (self.zoomPercent != 100) {
        self.zoomPercent = 100;
        [self rebuildVisualLines];
        if (self.onZoomChanged) self.onZoomChanged(100);
    }
}

@end

// ============================================================================
// Monitor Pane View (Single Serial Port Control + Log + TX)
// ============================================================================

@interface SsmPaneView : NSView <SsmSendFieldDelegate, NSComboBoxDelegate>
@property (nonatomic, assign) BOOL dedicated;
@property (nonatomic, copy) NSString *portName;
@property (nonatomic, assign) BOOL autoDetect;
@property (nonatomic, assign) NSInteger baud;
@property (nonatomic, assign) NSInteger dataBits;
@property (nonatomic, copy) NSString *parity;
@property (nonatomic, copy) NSString *stopBits;
@property (nonatomic, copy) NSString *txEol;
@property (nonatomic, copy) NSString *detectedEol;
@property (nonatomic, assign) BOOL connectedPref;
@property (nonatomic, assign) uint64_t lastSeq;
@property (nonatomic, copy) NSString *lastStateCode;
@property (nonatomic, assign) BOOL updatingUI;

@property (nonatomic, strong) NSMutableArray<NSString *> *comboPorts;
@property (nonatomic, strong) NSMutableArray<NSString *> *sendHistory;
@property (nonatomic, assign) NSInteger historyIdx;

// Controls
@property (nonatomic, strong) NSView *statusBg;
@property (nonatomic, strong) NSTextField *statusLabel;
@property (nonatomic, strong) NSPopUpButton *cmbPort;
@property (nonatomic, strong) NSButton *btnConnect;
@property (nonatomic, strong) NSButton *btnAuto;
@property (nonatomic, strong) NSButton *btnKill;
@property (nonatomic, strong) NSButton *btnPopout;

@property (nonatomic, strong) NSComboBox *cmbBaud;
@property (nonatomic, strong) NSPopUpButton *cmbDataBits;
@property (nonatomic, strong) NSPopUpButton *cmbParity;
@property (nonatomic, strong) NSPopUpButton *cmbStopBits;
@property (nonatomic, strong) NSPopUpButton *cmbEol;
@property (nonatomic, strong) NSPopUpButton *cmbDisplay;

@property (nonatomic, strong) NSButton *chkAutoScroll;
@property (nonatomic, strong) NSButton *chkTimestamp;
@property (nonatomic, strong) NSButton *chkWrap;
@property (nonatomic, strong) NSButton *btnClear;

@property (nonatomic, strong) SsmLogView *logView;
@property (nonatomic, strong) SsmSendField *editSend;
@property (nonatomic, strong) NSButton *btnSend;

@property (nonatomic, copy) void (^onConfigChanged)(void);
@property (nonatomic, copy) void (^onPopoutRequested)(NSDictionary *paneCfg);

- (instancetype)initWithConfig:(NSDictionary *)cfg dedicated:(BOOL)dedicated;
- (NSDictionary *)exportConfig;
- (void)refreshPortsList:(NSArray<NSDictionary *> *)ports;
- (void)pollTick;
- (void)applyLanguage;
- (void)applyAppearance;
- (void)cleanup;
@end

static NSArray<NSString *> *SsmFixedBauds(void) {
    return @[@"9600", @"19200", @"38400", @"57600", @"74880", @"115200", @"230400", @"460800", @"921600", @"1000000", @"2000000"];
}

@implementation SsmPaneView

- (BOOL)isFlipped {
    return YES;
}

- (NSButton *)makeCompactButton:(NSString *)title action:(SEL)action {
    NSButton *b = [[NSButton alloc] initWithFrame:NSZeroRect];
    b.title = title;
    b.bezelStyle = NSBezelStyleRounded;
    b.controlSize = NSControlSizeSmall;
    b.font = [NSFont systemFontOfSize:11.5];
    b.target = self;
    b.action = action;
    return b;
}

- (NSPopUpButton *)makeCompactPopup:(SEL)action {
    NSPopUpButton *p = [[NSPopUpButton alloc] initWithFrame:NSZeroRect pullsDown:NO];
    p.controlSize = NSControlSizeSmall;
    p.font = [NSFont systemFontOfSize:11.5];
    p.target = self;
    p.action = action;
    return p;
}

- (NSButton *)makeCompactCheckbox:(NSString *)title action:(SEL)action {
    NSButton *c = [NSButton checkboxWithTitle:title target:self action:action];
    c.controlSize = NSControlSizeSmall;
    c.font = [NSFont systemFontOfSize:11.5];
    return c;
}

- (instancetype)initWithConfig:(NSDictionary *)cfg dedicated:(BOOL)dedicated {
    self = [super initWithFrame:NSMakeRect(0, 0, 480, 320)];
    if (self) {
        self.wantsLayer = YES;
        self.layer.borderWidth = 1.0;

        self.dedicated = dedicated;
        self.comboPorts = [NSMutableArray array];
        self.sendHistory = [NSMutableArray array];
        self.historyIdx = -1;
        self.lastStateCode = @"";
        self.detectedEol = @"none";

        NSDictionary *ser = [cfg[@"serial"] isKindOfClass:[NSDictionary class]] ? cfg[@"serial"] : @{};
        self.portName = [cfg[@"port"] isKindOfClass:[NSString class]] ? cfg[@"port"] : @"";
        self.autoDetect = ser[@"auto_detect"] ? [ser[@"auto_detect"] boolValue] : YES;
        self.baud = ser[@"baud"] ? [ser[@"baud"] integerValue] : 115200;
        self.dataBits = ser[@"data_bits"] ? [ser[@"data_bits"] integerValue] : 8;
        self.parity = [ser[@"parity"] isKindOfClass:[NSString class]] ? ser[@"parity"] : @"N";
        self.stopBits = [ser[@"stop_bits"] isKindOfClass:[NSString class]] ? ser[@"stop_bits"] : @"1";
        self.txEol = [ser[@"tx_eol"] isKindOfClass:[NSString class]] ? ser[@"tx_eol"] : @"auto";
        self.connectedPref = cfg[@"connected"] ? [cfg[@"connected"] boolValue] : YES;

        // Status bar
        self.statusBg = [[NSView alloc] initWithFrame:NSZeroRect];
        self.statusBg.wantsLayer = YES;
        self.statusBg.layer.backgroundColor = SsmRGB(88, 96, 105).CGColor;
        [self addSubview:self.statusBg];

        self.statusLabel = [NSTextField labelWithString:@""];
        self.statusLabel.font = [NSFont systemFontOfSize:11.5 weight:NSFontWeightBold];
        self.statusLabel.textColor = [NSColor whiteColor];
        self.statusLabel.lineBreakMode = NSLineBreakByTruncatingTail;
        [self.statusBg addSubview:self.statusLabel];

        // Row 1 controls
        self.cmbPort = [self makeCompactPopup:@selector(onPortChanged:)];
        self.btnConnect = [self makeCompactButton:@"" action:@selector(onConnectClicked:)];
        self.btnAuto = [self makeCompactButton:@"" action:@selector(onAutoClicked:)];
        self.btnKill = [self makeCompactButton:@"" action:@selector(onKillClicked:)];
        self.btnPopout = [self makeCompactButton:@"" action:@selector(onPopoutClicked:)];
        self.btnPopout.hidden = dedicated;

        [self addSubview:self.cmbPort];
        [self addSubview:self.btnConnect];
        [self addSubview:self.btnAuto];
        [self addSubview:self.btnKill];
        [self addSubview:self.btnPopout];

        // Row 2 controls
        self.cmbBaud = [[NSComboBox alloc] initWithFrame:NSZeroRect];
        self.cmbBaud.controlSize = NSControlSizeSmall;
        self.cmbBaud.font = [NSFont systemFontOfSize:11.5];
        self.cmbBaud.delegate = self;
        self.cmbBaud.target = self;
        self.cmbBaud.action = @selector(onBaudCommitted:);
        self.cmbBaud.completes = NO;

        self.cmbDataBits = [self makeCompactPopup:@selector(onSerialParamChanged:)];
        [self.cmbDataBits addItemsWithTitles:@[@"8bit", @"7bit", @"6bit", @"5bit"]];

        self.cmbParity = [self makeCompactPopup:@selector(onSerialParamChanged:)];
        [self.cmbParity addItemsWithTitles:@[@"N", @"E", @"O", @"M", @"S"]];

        self.cmbStopBits = [self makeCompactPopup:@selector(onSerialParamChanged:)];
        [self.cmbStopBits addItemsWithTitles:@[@"1", @"1.5", @"2"]];

        self.cmbEol = [self makeCompactPopup:@selector(onSerialParamChanged:)];
        self.cmbDisplay = [self makeCompactPopup:@selector(onDisplayModeChanged:)];

        [self addSubview:self.cmbBaud];
        [self addSubview:self.cmbDataBits];
        [self addSubview:self.cmbParity];
        [self addSubview:self.cmbStopBits];
        [self addSubview:self.cmbEol];
        [self addSubview:self.cmbDisplay];

        // Row 3 controls
        self.chkAutoScroll = [self makeCompactCheckbox:@"" action:@selector(onCheckChanged:)];
        self.chkTimestamp = [self makeCompactCheckbox:@"" action:@selector(onCheckChanged:)];
        self.chkWrap = [self makeCompactCheckbox:@"" action:@selector(onCheckChanged:)];
        self.btnClear = [self makeCompactButton:@"" action:@selector(onClearClicked:)];

        [self addSubview:self.chkAutoScroll];
        [self addSubview:self.chkTimestamp];
        [self addSubview:self.chkWrap];
        [self addSubview:self.btnClear];

        // Log view
        self.logView = [[SsmLogView alloc] initWithFrame:NSZeroRect];
        self.logView.autoScroll = cfg[@"auto_scroll"] ? [cfg[@"auto_scroll"] boolValue] : YES;
        self.logView.showTimestamps = cfg[@"show_timestamps"] ? [cfg[@"show_timestamps"] boolValue] : NO;
        self.logView.wordWrap = cfg[@"word_wrap"] ? [cfg[@"word_wrap"] boolValue] : NO;
        self.logView.displayMode = [cfg[@"display_mode"] isKindOfClass:[NSString class]] ? cfg[@"display_mode"] : @"text";
        self.logView.zoomPercent = cfg[@"zoom_percent"] ? [cfg[@"zoom_percent"] integerValue] : 100;

        __weak typeof(self) weakSelf = self;
        self.logView.onAutoScrollChanged = ^(BOOL val) {
            weakSelf.chkAutoScroll.state = val ? NSControlStateValueOn : NSControlStateValueOff;
            if (weakSelf.onConfigChanged) weakSelf.onConfigChanged();
        };
        self.logView.onZoomChanged = ^(NSInteger z) {
            if (weakSelf.onConfigChanged) weakSelf.onConfigChanged();
        };
        [self addSubview:self.logView];

        // Send bar
        self.editSend = [[SsmSendField alloc] initWithFrame:NSZeroRect];
        self.editSend.sendDelegate = self;
        self.btnSend = [self makeCompactButton:@"" action:@selector(onSendClicked:)];
        [self addSubview:self.editSend];
        [self addSubview:self.btnSend];

        [self syncControlsFromState];
        [self applyLanguage];
        [self applyAppearance];

        if (self.portName.length > 0) {
            ssm_acquire_pane_ref(self.portName.UTF8String);
            ssm_attach_pane(self.portName.UTF8String, SsmToJsonString([self serialConfigDict]).UTF8String, self.connectedPref ? 1 : 0);
        }
    }
    return self;
}

- (void)cleanup {
    if (self.portName.length > 0) {
        ssm_release_pane_ref(self.portName.UTF8String);
        self.portName = @"";
    }
}

- (NSDictionary *)serialConfigDict {
    return @{
        @"auto_detect": @(self.autoDetect),
        @"baud": @(self.baud),
        @"data_bits": @(self.dataBits),
        @"parity": self.parity ?: @"N",
        @"stop_bits": self.stopBits ?: @"1",
        @"tx_eol": self.txEol ?: @"auto"
    };
}

- (NSDictionary *)exportConfig {
    return @{
        @"port": self.portName ?: @"",
        @"serial": [self serialConfigDict],
        @"auto_scroll": @(self.logView.autoScroll),
        @"show_timestamps": @(self.logView.showTimestamps),
        @"word_wrap": @(self.logView.wordWrap),
        @"display_mode": self.logView.displayMode ?: @"text",
        @"zoom_percent": @(self.logView.zoomPercent),
        @"connected": @(self.connectedPref)
    };
}

- (void)syncBaudCombo {
    self.updatingUI = YES;
    NSString *autoLabel = [NSString stringWithFormat:@"Auto (%ld)", (long)self.baud];
    [self.cmbBaud removeAllItems];
    [self.cmbBaud addItemWithObjectValue:autoLabel];
    for (NSString *b in SsmFixedBauds()) {
        [self.cmbBaud addItemWithObjectValue:b];
    }
    if (self.autoDetect) {
        self.cmbBaud.stringValue = autoLabel;
    } else {
        self.cmbBaud.stringValue = [NSString stringWithFormat:@"%ld", (long)self.baud];
    }
    self.updatingUI = NO;
}

- (void)syncEolPopup {
    self.updatingUI = YES;
    NSString *autoEol = [self.detectedEol isEqualToString:@"none"] || self.detectedEol.length == 0
        ? @"Auto"
        : [NSString stringWithFormat:@"Auto (%@)", SsmEolLabel(self.detectedEol)];
    [self.cmbEol removeAllItems];
    [self.cmbEol addItemsWithTitles:@[autoEol, @"CRLF", @"LF", @"CR", @"None"]];
    NSInteger idx = 0;
    if ([self.txEol isEqualToString:@"crlf"]) idx = 1;
    else if ([self.txEol isEqualToString:@"lf"]) idx = 2;
    else if ([self.txEol isEqualToString:@"cr"]) idx = 3;
    else if ([self.txEol isEqualToString:@"none"]) idx = 4;
    [self.cmbEol selectItemAtIndex:idx];
    self.updatingUI = NO;
}

- (void)syncControlsFromState {
    self.updatingUI = YES;
    [self syncBaudCombo];

    NSInteger dbIdx = 0;
    if (self.dataBits == 7) dbIdx = 1;
    else if (self.dataBits == 6) dbIdx = 2;
    else if (self.dataBits == 5) dbIdx = 3;
    [self.cmbDataBits selectItemAtIndex:dbIdx];

    NSArray *parList = @[@"N", @"E", @"O", @"M", @"S"];
    NSUInteger pIdx = [parList indexOfObject:self.parity.uppercaseString];
    [self.cmbParity selectItemAtIndex:(pIdx == NSNotFound ? 0 : (NSInteger)pIdx)];

    NSInteger sbIdx = 0;
    if ([self.stopBits isEqualToString:@"1.5"]) sbIdx = 1;
    else if ([self.stopBits isEqualToString:@"2"]) sbIdx = 2;
    [self.cmbStopBits selectItemAtIndex:sbIdx];

    [self syncEolPopup];

    self.chkAutoScroll.state = self.logView.autoScroll ? NSControlStateValueOn : NSControlStateValueOff;
    self.chkTimestamp.state = self.logView.showTimestamps ? NSControlStateValueOn : NSControlStateValueOff;
    self.chkWrap.state = self.logView.wordWrap ? NSControlStateValueOn : NSControlStateValueOff;
    self.updatingUI = NO;
}

- (void)applyLanguage {
    self.updatingUI = YES;
    BOOL isReleased = [self.lastStateCode isEqualToString:@"released"] || !self.connectedPref;
    self.btnConnect.title = isReleased ? TR(@"Connect", @"接続") : TR(@"Disconnect", @"切断");
    self.btnAuto.title = TR(@"Auto", @"Auto検出");
    self.btnKill.title = TR(@"Kill Owner", @"占有終了");
    self.btnPopout.title = TR(@"Pop Out", @"別窓");
    self.chkAutoScroll.title = TR(@"Auto-scroll", @"自動スクロール");
    self.chkTimestamp.title = TR(@"Time", @"時刻");
    self.chkWrap.title = TR(@"Wrap", @"折り返し");
    self.btnClear.title = TR(@"Clear", @"クリア");
    self.btnSend.title = TR(@"Send", @"送信");
    self.editSend.placeholderString = TR(@"Send data (Enter to send, ↑/↓ history)", @"送信データ (Enterで送信 / ↑↓履歴)");

    [self.cmbDisplay removeAllItems];
    [self.cmbDisplay addItemsWithTitles:@[TR(@"Text", @"テキスト"), @"HEX", @"BIN"]];
    NSInteger dmIdx = 0;
    if ([self.logView.displayMode isEqualToString:@"hex"]) dmIdx = 1;
    else if ([self.logView.displayMode isEqualToString:@"bin"]) dmIdx = 2;
    [self.cmbDisplay selectItemAtIndex:dmIdx];

    // Tooltips
    self.cmbPort.toolTip = TR(@"Select serial port", @"監視するシリアルポートを選択");
    self.btnConnect.toolTip = TR(@"Connect or release this port", @"ポートの接続 / 一時解放（書き込み前など）");
    self.btnAuto.toolTip = TR(@"Re-run baud & framing auto-detection", @"ボーレート・フレーム設定を自動検出");
    self.btnKill.toolTip = TR(@"Terminate process holding this port", @"ポートを占有中の他プロセスを終了して接続");
    self.btnPopout.toolTip = TR(@"Open this port in a dedicated window", @"このポート専用の別ウィンドウを開く");
    self.cmbBaud.toolTip = TR(@"Baud rate (Auto or custom number)", @"ボーレート（Autoまたはカスタム値手入力）");
    self.cmbDataBits.toolTip = TR(@"Data bits", @"データビット");
    self.cmbParity.toolTip = TR(@"Parity (N/E/O/M/S)", @"パリティ (N/E/O/M/S)");
    self.cmbStopBits.toolTip = TR(@"Stop bits", @"ストップビット");
    self.cmbEol.toolTip = TR(@"TX line ending", @"送信時の改行コード");
    self.cmbDisplay.toolTip = TR(@"Display mode (Text / HEX / BIN)", @"表示形式（テキスト / HEX / BIN）");
    self.btnClear.toolTip = TR(@"Clear log buffer", @"ログ表示とログファイルをクリア");
    self.updatingUI = NO;

    if (self.portName.length == 0) {
        self.statusLabel.stringValue = [NSString stringWithFormat:@" ○ %@", TR(@"No port selected", @"ポート未選択")];
    }
}

- (void)applyAppearance {
    BOOL dark = SsmIsDarkActive();
    self.layer.backgroundColor = (dark ? SsmRGB(30, 33, 40) : SsmRGB(243, 244, 246)).CGColor;
    self.layer.borderColor = (dark ? SsmRGB(54, 60, 72) : SsmRGB(206, 212, 220)).CGColor;
    [self.logView refreshAppearance];
}

- (void)layout {
    [super layout];
    CGFloat w = self.bounds.size.width;
    CGFloat h = self.bounds.size.height;
    CGFloat pad = 5.0;
    CGFloat y = 0.0;

    // Status bar
    CGFloat statusH = 22.0;
    self.statusBg.frame = NSMakeRect(0, y, w, statusH);
    self.statusLabel.frame = NSMakeRect(6, 3, MAX(10, w - 12), 16);
    y += statusH + 4.0;

    // Row 1: Port + Connect + Auto + Kill + Popout
    CGFloat rowH = 24.0;
    CGFloat popW = self.dedicated ? 0.0 : 54.0;
    CGFloat killW = 68.0;
    CGFloat autoW = 66.0;
    CGFloat connW = 54.0;
    CGFloat rightUsed = connW + autoW + killW + popW + (self.dedicated ? 12.0 : 16.0);
    CGFloat portW = MAX(80.0, w - pad * 2 - rightUsed);

    CGFloat x = pad;
    self.cmbPort.frame = NSMakeRect(x, y, portW, rowH);
    x += portW + 4.0;
    self.btnConnect.frame = NSMakeRect(x, y, connW, rowH);
    x += connW + 4.0;
    self.btnAuto.frame = NSMakeRect(x, y, autoW, rowH);
    x += autoW + 4.0;
    self.btnKill.frame = NSMakeRect(x, y, killW, rowH);
    if (!self.dedicated) {
        x += killW + 4.0;
        self.btnPopout.frame = NSMakeRect(x, y, popW, rowH);
    }
    y += rowH + 4.0;

    // Row 2: Baud + DataBits + Parity + StopBits + EOL + Display
    x = pad;
    CGFloat baudW = MIN(118.0, MAX(84.0, w * 0.24));
    self.cmbBaud.frame = NSMakeRect(x, y, baudW, rowH);
    x += baudW + 4.0;
    self.cmbDataBits.frame = NSMakeRect(x, y, 56.0, rowH);
    x += 60.0;
    self.cmbParity.frame = NSMakeRect(x, y, 42.0, rowH);
    x += 46.0;
    self.cmbStopBits.frame = NSMakeRect(x, y, 46.0, rowH);
    x += 50.0;
    CGFloat eolW = 86.0;
    self.cmbEol.frame = NSMakeRect(x, y, eolW, rowH);
    x += eolW + 4.0;
    CGFloat dispW = MIN(88.0, MAX(56.0, w - pad - x));
    self.cmbDisplay.frame = NSMakeRect(x, y, dispW, rowH);
    y += rowH + 3.0;

    // Row 3: Checkboxes + Clear
    CGFloat optH = 20.0;
    x = pad;
    self.chkAutoScroll.frame = NSMakeRect(x, y, 108.0, optH);
    x += 110.0;
    self.chkTimestamp.frame = NSMakeRect(x, y, 56.0, optH);
    x += 58.0;
    self.chkWrap.frame = NSMakeRect(x, y, 76.0, optH);
    CGFloat clearW = 56.0;
    self.btnClear.frame = NSMakeRect(MAX(x + 4.0, w - pad - clearW), y - 1.0, clearW, 22.0);
    y += optH + 4.0;

    // Bottom send bar
    CGFloat sendH = 24.0;
    CGFloat sendY = MAX(y + 40.0, h - pad - sendH);
    CGFloat sendBtnW = 56.0;
    self.editSend.frame = NSMakeRect(pad, sendY, MAX(60.0, w - pad * 2 - sendBtnW - 4.0), sendH);
    self.btnSend.frame = NSMakeRect(w - pad - sendBtnW, sendY, sendBtnW, sendH);

    // Log view fills the middle
    CGFloat logH = MAX(30.0, sendY - 4.0 - y);
    self.logView.frame = NSMakeRect(pad, y, MAX(40.0, w - pad * 2), logH);
}

- (void)refreshPortsList:(NSArray<NSDictionary *> *)ports {
    self.updatingUI = YES;
    [self.comboPorts removeAllObjects];
    [self.cmbPort removeAllItems];

    [self.comboPorts addObject:@""];
    [self.cmbPort addItemWithTitle:TR(@"(Select Port)", @"(ポート未選択)")];

    BOOL foundCurrent = NO;
    for (NSDictionary *p in ports) {
        NSString *name = p[@"port"] ?: @"";
        NSString *hw = p[@"hardware_name"] ?: @"";
        if (name.length == 0) continue;
        [self.comboPorts addObject:name];
        [self.cmbPort addItemWithTitle:SsmShortPortLabel(name, hw)];
        if ([name isEqualToString:self.portName]) {
            foundCurrent = YES;
        }
    }
    if (self.portName.length > 0 && !foundCurrent) {
        [self.comboPorts addObject:self.portName];
        NSString *lbl = [NSString stringWithFormat:@"%@ (%@)",
            SsmShortPortLabel(self.portName, @""),
            TR(@"Unplugged", @"未接続")];
        [self.cmbPort addItemWithTitle:lbl];
    }

    NSUInteger idx = [self.comboPorts indexOfObject:(self.portName ?: @"")];
    [self.cmbPort selectItemAtIndex:(idx == NSNotFound ? 0 : (NSInteger)idx)];
    self.updatingUI = NO;
}

- (void)switchToPort:(NSString *)newPort {
    if ([newPort isEqualToString:self.portName]) return;
    if (self.portName.length > 0) {
        ssm_release_pane_ref(self.portName.UTF8String);
    }
    self.portName = newPort ?: @"";
    self.lastSeq = 0;
    self.lastStateCode = @"";
    [self.logView clearAll];

    if (self.portName.length > 0) {
        self.connectedPref = YES;
        ssm_acquire_pane_ref(self.portName.UTF8String);
        ssm_attach_pane(self.portName.UTF8String, SsmToJsonString([self serialConfigDict]).UTF8String, 1);
    } else {
        self.statusBg.layer.backgroundColor = SsmRGB(88, 96, 105).CGColor;
        self.statusLabel.stringValue = [NSString stringWithFormat:@" ○ %@", TR(@"No port selected", @"ポート未選択")];
    }
    if (self.onConfigChanged) self.onConfigChanged();
}

- (void)pollTick {
    if (self.portName.length == 0) return;
    NSDictionary *resp = SsmParseJsonC(ssm_poll_pane_json(self.portName.UTF8String, self.lastSeq, self.lastSeq == 0 ? 500 : 0));
    if (![resp isKindOfClass:[NSDictionary class]] || ![resp[@"has_session"] boolValue]) return;

    // Update serial config if auto-detect changed it
    NSDictionary *cfg = resp[@"config"];
    if ([cfg isKindOfClass:[NSDictionary class]]) {
        BOOL newAuto = [cfg[@"auto_detect"] boolValue];
        NSInteger newBaud = [cfg[@"baud"] integerValue];
        NSInteger newBits = [cfg[@"data_bits"] integerValue];
        NSString *newParity = cfg[@"parity"] ?: @"N";
        NSString *newStop = cfg[@"stop_bits"] ?: @"1";
        NSString *detEol = resp[@"detected_eol"] ?: @"none";

        BOOL changed = (newAuto != self.autoDetect || newBaud != self.baud || newBits != self.dataBits ||
                        ![newParity isEqualToString:self.parity] || ![newStop isEqualToString:self.stopBits]);
        if (changed) {
            self.autoDetect = newAuto;
            self.baud = newBaud;
            self.dataBits = newBits;
            self.parity = newParity;
            self.stopBits = newStop;
            [self syncControlsFromState];
        }
        if (![detEol isEqualToString:self.detectedEol]) {
            self.detectedEol = detEol;
            [self syncEolPopup];
        }
    }

    // Update status bar
    NSDictionary *st = resp[@"state"];
    NSString *code = [st isKindOfClass:[NSDictionary class]] ? (st[@"code"] ?: @"released") : @"released";
    BOOL autoLocked = [resp[@"auto_locked"] boolValue];
    NSString *hw = resp[@"hardware_label"] ?: @"";
    NSString *shortPort = [self.portName hasPrefix:@"/dev/"] ? [self.portName substringFromIndex:5] : self.portName;
    NSString *hwSuffix = hw.length > 0 ? [NSString stringWithFormat:@" | %@", hw] : @"";
    NSString *framing = [NSString stringWithFormat:@"%ld%@%@", (long)self.dataBits, self.parity, self.stopBits];
    NSString *eolTag = [self.txEol isEqualToString:@"auto"]
        ? ([self.detectedEol isEqualToString:@"none"] ? @"Auto" : SsmEolLabel(self.detectedEol))
        : SsmEolLabel(self.txEol);

    NSColor *barColor = SsmRGB(88, 96, 105);
    NSString *statusText = @"";
    if ([code isEqualToString:@"connected"]) {
        barColor = SsmRGB(27, 118, 56);
        NSString *autoTag = self.autoDetect ? (autoLocked ? TR(@" [Auto locked]", @" [Auto確定]") : @" [Auto]") : @"";
        statusText = [NSString stringWithFormat:@" ● %@ %@ %ld %@ %@%@%@",
            TR(@"Connected", @"接続中"), shortPort, (long)self.baud, framing, eolTag, autoTag, hwSuffix];
    } else if ([code isEqualToString:@"autodetecting"]) {
        barColor = SsmRGB(25, 103, 210);
        NSInteger probing = [st[@"probing_baud"] integerValue];
        statusText = [NSString stringWithFormat:@" ◎ %@ %@ (%ld bps...)%@",
            TR(@"Auto-detecting", @"Auto検出中"), shortPort, (long)probing, hwSuffix];
    } else if ([code isEqualToString:@"occupied"]) {
        barColor = SsmRGB(198, 40, 40);
        NSDictionary *owner = [st[@"owner"] isKindOfClass:[NSDictionary class]] ? st[@"owner"] : nil;
        if (owner) {
            statusText = [NSString stringWithFormat:@" ▲ %@ %@ — %@ (PID %@)%@",
                TR(@"In Use", @"占有中"), shortPort, owner[@"name"] ?: @"?", owner[@"pid"] ?: @"?", hwSuffix];
        } else {
            statusText = [NSString stringWithFormat:@" ▲ %@ %@%@", TR(@"In Use", @"占有中"), shortPort, hwSuffix];
        }
    } else if ([code isEqualToString:@"disconnected"] || [code isEqualToString:@"connecting"]) {
        barColor = SsmRGB(176, 104, 0);
        NSString *reason = st[@"reason"] ?: TR(@"Connecting...", @"接続中...");
        statusText = [NSString stringWithFormat:@" ◌ %@ %@ — %@%@",
            TR(@"Waiting", @"再接続待機"), shortPort, reason, hwSuffix];
    } else {
        barColor = SsmRGB(88, 96, 105);
        statusText = [NSString stringWithFormat:@" ○ %@ %@%@",
            TR(@"Released", @"切断中 (解放)"), shortPort, hwSuffix];
    }

    self.statusBg.layer.backgroundColor = barColor.CGColor;
    self.statusLabel.stringValue = statusText;
    if (![code isEqualToString:self.lastStateCode]) {
        self.lastStateCode = code;
        BOOL isReleased = [code isEqualToString:@"released"];
        self.btnConnect.title = isReleased ? TR(@"Connect", @"接続") : TR(@"Disconnect", @"切断");
    }

    // Append new logs
    NSArray *entries = resp[@"entries"];
    if ([entries isKindOfClass:[NSArray class]] && entries.count > 0) {
        [self.logView appendEntries:entries];
        NSDictionary *last = entries.lastObject;
        uint64_t seq = [last[@"seq"] unsignedLongLongValue];
        if (seq > self.lastSeq) self.lastSeq = seq;
    }
    uint64_t latestSeq = [resp[@"latest_seq"] unsignedLongLongValue];
    if (latestSeq < self.lastSeq) {
        // Logs were cleared externally (e.g. via CLI/MCP)
        self.lastSeq = 0;
        [self.logView clearAll];
    }
}

// Actions
- (void)onPortChanged:(id)sender {
    if (self.updatingUI) return;
    NSInteger idx = self.cmbPort.indexOfSelectedItem;
    if (idx >= 0 && idx < (NSInteger)self.comboPorts.count) {
        [self switchToPort:self.comboPorts[idx]];
    }
}

- (void)onConnectClicked:(id)sender {
    if (self.portName.length == 0) return;
    if ([self.lastStateCode isEqualToString:@"released"]) {
        self.connectedPref = YES;
        ssm_attach_pane(self.portName.UTF8String, SsmToJsonString([self serialConfigDict]).UTF8String, 1);
        ssm_connect(self.portName.UTF8String);
    } else {
        self.connectedPref = NO;
        ssm_disconnect(self.portName.UTF8String);
    }
    if (self.onConfigChanged) self.onConfigChanged();
}

- (void)onAutoClicked:(id)sender {
    if (self.portName.length == 0) return;
    self.autoDetect = YES;
    self.connectedPref = YES;
    ssm_autodetect(self.portName.UTF8String);
    [self syncBaudCombo];
    if (self.onConfigChanged) self.onConfigChanged();
}

- (void)onKillClicked:(id)sender {
    if (self.portName.length == 0) return;
    self.connectedPref = YES;
    ssm_kill_owner(self.portName.UTF8String);
}

- (void)onPopoutClicked:(id)sender {
    if (self.onPopoutRequested) {
        self.onPopoutRequested([self exportConfig]);
    }
}

- (void)commitBaudString:(NSString *)raw {
    if (self.updatingUI) return;
    NSString *trimmed = [raw stringByTrimmingCharactersInSet:[NSCharacterSet whitespaceAndNewlineCharacterSet]];
    if ([[trimmed lowercaseString] hasPrefix:@"auto"]) {
        self.autoDetect = YES;
    } else {
        NSInteger val = [trimmed integerValue];
        if (val >= 110 && val <= 4000000) {
            self.autoDetect = NO;
            self.baud = val;
        }
    }
    [self syncBaudCombo];
    if (self.portName.length > 0) {
        ssm_update_serial_config(self.portName.UTF8String, SsmToJsonString([self serialConfigDict]).UTF8String);
    }
    if (self.onConfigChanged) self.onConfigChanged();
}

- (void)onBaudCommitted:(id)sender {
    [self commitBaudString:self.cmbBaud.stringValue ?: @""];
}

- (void)comboBoxSelectionDidChange:(NSNotification *)notification {
    NSInteger idx = self.cmbBaud.indexOfSelectedItem;
    if (idx >= 0) {
        NSString *val = [self.cmbBaud itemObjectValueAtIndex:idx];
        [self commitBaudString:val ?: @""];
    }
}

- (void)onSerialParamChanged:(id)sender {
    if (self.updatingUI) return;
    NSArray<NSNumber *> *bits = @[@8, @7, @6, @5];
    NSInteger dbIdx = self.cmbDataBits.indexOfSelectedItem;
    if (dbIdx >= 0 && dbIdx < (NSInteger)bits.count) self.dataBits = bits[dbIdx].integerValue;

    NSArray<NSString *> *pars = @[@"N", @"E", @"O", @"M", @"S"];
    NSInteger pIdx = self.cmbParity.indexOfSelectedItem;
    if (pIdx >= 0 && pIdx < (NSInteger)pars.count) self.parity = pars[pIdx];

    NSArray<NSString *> *stops = @[@"1", @"1.5", @"2"];
    NSInteger sIdx = self.cmbStopBits.indexOfSelectedItem;
    if (sIdx >= 0 && sIdx < (NSInteger)stops.count) self.stopBits = stops[sIdx];

    NSArray<NSString *> *eols = @[@"auto", @"crlf", @"lf", @"cr", @"none"];
    NSInteger eIdx = self.cmbEol.indexOfSelectedItem;
    if (eIdx >= 0 && eIdx < (NSInteger)eols.count) self.txEol = eols[eIdx];

    if (sender == self.cmbDataBits || sender == self.cmbParity || sender == self.cmbStopBits) {
        self.autoDetect = NO;
        [self syncBaudCombo];
    }
    if (self.portName.length > 0) {
        ssm_update_serial_config(self.portName.UTF8String, SsmToJsonString([self serialConfigDict]).UTF8String);
    }
    if (self.onConfigChanged) self.onConfigChanged();
}

- (void)onDisplayModeChanged:(id)sender {
    if (self.updatingUI) return;
    NSArray<NSString *> *modes = @[@"text", @"hex", @"bin"];
    NSInteger idx = self.cmbDisplay.indexOfSelectedItem;
    if (idx >= 0 && idx < (NSInteger)modes.count) {
        self.logView.displayMode = modes[idx];
        [self.logView rebuildVisualLines];
        if (self.onConfigChanged) self.onConfigChanged();
    }
}

- (void)onCheckChanged:(id)sender {
    if (self.updatingUI) return;
    self.logView.autoScroll = (self.chkAutoScroll.state == NSControlStateValueOn);
    self.logView.showTimestamps = (self.chkTimestamp.state == NSControlStateValueOn);
    self.logView.wordWrap = (self.chkWrap.state == NSControlStateValueOn);
    [self.logView rebuildVisualLines];
    if (self.logView.autoScroll) {
        [self.logView scrollToBottom];
    }
    if (self.onConfigChanged) self.onConfigChanged();
}

- (void)onClearClicked:(id)sender {
    [self.logView clearAll];
    self.lastSeq = 0;
    if (self.portName.length > 0) {
        ssm_clear_logs(self.portName.UTF8String);
    }
}

- (void)onSendClicked:(id)sender {
    [self sendFieldDidSubmit:self.editSend.stringValue ?: @""];
}

- (void)sendFieldDidSubmit:(NSString *)text {
    if (self.portName.length == 0) return;
    ssm_send_text(self.portName.UTF8String, text.UTF8String);
    if (text.length > 0) {
        if (self.sendHistory.count == 0 || ![self.sendHistory.lastObject isEqualToString:text]) {
            [self.sendHistory addObject:text];
            if (self.sendHistory.count > 100) [self.sendHistory removeObjectAtIndex:0];
        }
    }
    self.historyIdx = -1;
    self.editSend.stringValue = @"";
}

- (void)sendFieldHistoryPrev {
    if (self.sendHistory.count == 0) return;
    if (self.historyIdx < 0) {
        self.historyIdx = (NSInteger)self.sendHistory.count - 1;
    } else if (self.historyIdx > 0) {
        self.historyIdx--;
    }
    self.editSend.stringValue = self.sendHistory[self.historyIdx];
}

- (void)sendFieldHistoryNext {
    if (self.sendHistory.count == 0 || self.historyIdx < 0) return;
    if (self.historyIdx + 1 < (NSInteger)self.sendHistory.count) {
        self.historyIdx++;
        self.editSend.stringValue = self.sendHistory[self.historyIdx];
    } else {
        self.historyIdx = -1;
        self.editSend.stringValue = @"";
    }
}

@end

// ============================================================================
// Draggable Multi-Pane Grid Container (1x1, 1x2, 2x1, 2x2)
// ============================================================================

typedef NS_ENUM(NSInteger, SsmDragAxis) {
    SsmDragNone = 0,
    SsmDragHoriz,
    SsmDragVert,
    SsmDragBoth
};

@interface SsmGridContainerView : NSView
@property (nonatomic, copy) NSString *layoutMode; // 1x1 | 1x2 | 2x1 | 2x2
@property (nonatomic, assign) CGFloat splitX;
@property (nonatomic, assign) CGFloat splitY;
@property (nonatomic, strong) NSMutableArray<SsmPaneView *> *panes;
@property (nonatomic, assign) SsmDragAxis dragAxis;
@property (nonatomic, copy) void (^onSplitChanged)(void);
@end

@implementation SsmGridContainerView

- (BOOL)isFlipped {
    return YES;
}

- (instancetype)initWithFrame:(NSRect)frameRect {
    self = [super initWithFrame:frameRect];
    if (self) {
        self.layoutMode = @"1x1";
        self.splitX = 0.5;
        self.splitY = 0.5;
        self.panes = [NSMutableArray array];
    }
    return self;
}

- (NSInteger)visiblePaneCount {
    if ([self.layoutMode isEqualToString:@"1x2"] || [self.layoutMode isEqualToString:@"2x1"]) return 2;
    if ([self.layoutMode isEqualToString:@"2x2"]) return 4;
    return 1;
}

- (void)layout {
    [super layout];
    CGFloat w = self.bounds.size.width;
    CGFloat h = self.bounds.size.height;
    CGFloat gap = 6.0;

    NSInteger visible = [self visiblePaneCount];
    for (NSInteger i = 0; i < (NSInteger)self.panes.count; i++) {
        self.panes[i].hidden = (i >= visible);
    }
    if (self.panes.count < 4) return;

    if ([self.layoutMode isEqualToString:@"1x1"]) {
        self.panes[0].frame = NSMakeRect(0, 0, w, h);
    } else if ([self.layoutMode isEqualToString:@"1x2"]) {
        CGFloat lw = floor((w - gap) * self.splitX);
        CGFloat rw = MAX(60.0, w - gap - lw);
        self.panes[0].frame = NSMakeRect(0, 0, lw, h);
        self.panes[1].frame = NSMakeRect(lw + gap, 0, rw, h);
    } else if ([self.layoutMode isEqualToString:@"2x1"]) {
        CGFloat th = floor((h - gap) * self.splitY);
        CGFloat bh = MAX(60.0, h - gap - th);
        self.panes[0].frame = NSMakeRect(0, 0, w, th);
        self.panes[1].frame = NSMakeRect(0, th + gap, w, bh);
    } else {
        CGFloat lw = floor((w - gap) * self.splitX);
        CGFloat rw = MAX(60.0, w - gap - lw);
        CGFloat th = floor((h - gap) * self.splitY);
        CGFloat bh = MAX(60.0, h - gap - th);
        self.panes[0].frame = NSMakeRect(0, 0, lw, th);
        self.panes[1].frame = NSMakeRect(lw + gap, 0, rw, th);
        self.panes[2].frame = NSMakeRect(0, th + gap, lw, bh);
        self.panes[3].frame = NSMakeRect(lw + gap, th + gap, rw, bh);
    }
    [self setNeedsDisplay:YES];
}

- (void)drawRect:(NSRect)dirtyRect {
    BOOL dark = SsmIsDarkActive();
    [(dark ? SsmRGB(38, 42, 52) : SsmRGB(228, 232, 238)) setFill];
    NSRectFill(dirtyRect);

    if ([self.layoutMode isEqualToString:@"1x1"]) return;
    CGFloat w = self.bounds.size.width;
    CGFloat h = self.bounds.size.height;
    CGFloat gap = 6.0;
    NSColor *grip = dark ? SsmRGB(96, 106, 124) : SsmRGB(165, 173, 184);
    [grip setFill];

    if ([self.layoutMode isEqualToString:@"1x2"] || [self.layoutMode isEqualToString:@"2x2"]) {
        CGFloat sx = floor((w - gap) * self.splitX) + 2.0;
        NSRectFill(NSMakeRect(sx, h * 0.5 - 16.0, 2.0, 32.0));
    }
    if ([self.layoutMode isEqualToString:@"2x1"] || [self.layoutMode isEqualToString:@"2x2"]) {
        CGFloat sy = floor((h - gap) * self.splitY) + 2.0;
        NSRectFill(NSMakeRect(w * 0.5 - 16.0, sy, 32.0, 2.0));
    }
}

- (SsmDragAxis)hitAxisForPoint:(NSPoint)pt {
    CGFloat w = self.bounds.size.width;
    CGFloat h = self.bounds.size.height;
    CGFloat gap = 6.0;
    BOOL hitX = NO, hitY = NO;
    if ([self.layoutMode isEqualToString:@"1x2"] || [self.layoutMode isEqualToString:@"2x2"]) {
        CGFloat sx = floor((w - gap) * self.splitX);
        if (pt.x >= sx - 3.0 && pt.x <= sx + gap + 3.0) hitX = YES;
    }
    if ([self.layoutMode isEqualToString:@"2x1"] || [self.layoutMode isEqualToString:@"2x2"]) {
        CGFloat sy = floor((h - gap) * self.splitY);
        if (pt.y >= sy - 3.0 && pt.y <= sy + gap + 3.0) hitY = YES;
    }
    if (hitX && hitY) return SsmDragBoth;
    if (hitX) return SsmDragHoriz;
    if (hitY) return SsmDragVert;
    return SsmDragNone;
}

- (void)resetCursorRects {
    [super resetCursorRects];
    CGFloat w = self.bounds.size.width;
    CGFloat h = self.bounds.size.height;
    CGFloat gap = 6.0;
    if ([self.layoutMode isEqualToString:@"1x2"] || [self.layoutMode isEqualToString:@"2x2"]) {
        CGFloat sx = floor((w - gap) * self.splitX);
        [self addCursorRect:NSMakeRect(sx, 0, gap, h) cursor:[NSCursor resizeLeftRightCursor]];
    }
    if ([self.layoutMode isEqualToString:@"2x1"] || [self.layoutMode isEqualToString:@"2x2"]) {
        CGFloat sy = floor((h - gap) * self.splitY);
        [self addCursorRect:NSMakeRect(0, sy, w, gap) cursor:[NSCursor resizeUpDownCursor]];
    }
}

- (void)mouseDown:(NSEvent *)event {
    NSPoint pt = [self convertPoint:event.locationInWindow fromView:nil];
    self.dragAxis = [self hitAxisForPoint:pt];
}

- (void)mouseDragged:(NSEvent *)event {
    if (self.dragAxis == SsmDragNone) return;
    NSPoint pt = [self convertPoint:event.locationInWindow fromView:nil];
    CGFloat w = MAX(100.0, self.bounds.size.width);
    CGFloat h = MAX(100.0, self.bounds.size.height);
    if (self.dragAxis == SsmDragHoriz || self.dragAxis == SsmDragBoth) {
        self.splitX = MIN(0.85, MAX(0.15, pt.x / w));
    }
    if (self.dragAxis == SsmDragVert || self.dragAxis == SsmDragBoth) {
        self.splitY = MIN(0.85, MAX(0.15, pt.y / h));
    }
    self.needsLayout = YES;
    [self layoutSubtreeIfNeeded];
    [self.window invalidateCursorRectsForView:self];
}

- (void)mouseUp:(NSEvent *)event {
    if (self.dragAxis != SsmDragNone) {
        self.dragAxis = SsmDragNone;
        if (self.onSplitChanged) self.onSplitChanged();
    }
}

@end

// ============================================================================
// Toast Popup Panel (In-App Device Notification)
// ============================================================================

@interface SsmToastPanel : NSPanel
+ (void)showNearWindow:(NSWindow *)owner title:(NSString *)title body:(NSString *)body;
@end

@implementation SsmToastPanel

- (void)mouseDown:(NSEvent *)event {
    [self orderOut:nil];
}

+ (void)showNearWindow:(NSWindow *)owner title:(NSString *)title body:(NSString *)body {
    NSArray *lines = [body componentsSeparatedByString:@"\n"];
    NSInteger lineCount = MIN(4, MAX(1, (NSInteger)lines.count));
    CGFloat w = 340.0;
    CGFloat h = 44.0 + lineCount * 17.0;

    NSRect base = owner ? owner.frame : [[NSScreen mainScreen] visibleFrame];
    CGFloat x = NSMaxX(base) - w - 18.0;
    CGFloat y = NSMinY(base) + 18.0;

    SsmToastPanel *panel = [[SsmToastPanel alloc] initWithContentRect:NSMakeRect(x, y, w, h)
                                                            styleMask:NSWindowStyleMaskBorderless | NSWindowStyleMaskNonactivatingPanel
                                                              backing:NSBackingStoreBuffered
                                                                defer:NO];
    panel.level = NSFloatingWindowLevel;
    panel.opaque = NO;
    panel.backgroundColor = [NSColor clearColor];
    panel.hasShadow = YES;
    panel.releasedWhenClosed = NO;

    NSView *box = [[NSView alloc] initWithFrame:NSMakeRect(0, 0, w, h)];
    box.wantsLayer = YES;
    box.layer.backgroundColor = SsmRGB(28, 34, 44).CGColor;
    box.layer.borderColor = SsmRGB(64, 152, 236).CGColor;
    box.layer.borderWidth = 1.5;
    box.layer.cornerRadius = 8.0;

    NSTextField *lblTitle = [NSTextField labelWithString:title];
    lblTitle.frame = NSMakeRect(12, h - 24, w - 24, 18);
    lblTitle.font = [NSFont systemFontOfSize:12.5 weight:NSFontWeightBold];
    lblTitle.textColor = [NSColor whiteColor];
    [box addSubview:lblTitle];

    NSTextField *lblBody = [NSTextField wrappingLabelWithString:body];
    lblBody.frame = NSMakeRect(12, 8, w - 24, h - 34);
    lblBody.font = [NSFont systemFontOfSize:11.5];
    lblBody.textColor = SsmRGB(210, 222, 236);
    [box addSubview:lblBody];

    panel.contentView = box;
    [panel orderFrontRegardless];

    dispatch_after(dispatch_time(DISPATCH_TIME_NOW, (int64_t)(4.5 * NSEC_PER_SEC)), dispatch_get_main_queue(), ^{
        [panel orderOut:nil];
    });
}

@end

// ============================================================================
// Settings Window Controller
// ============================================================================

@interface SsmSettingsWindowController : NSWindowController
@property (nonatomic, strong) NSTextField *lblLang;
@property (nonatomic, strong) NSPopUpButton *cmbLang;
@property (nonatomic, strong) NSTextField *lblTheme;
@property (nonatomic, strong) NSPopUpButton *cmbTheme;
@property (nonatomic, strong) NSTextField *lblNotify;
@property (nonatomic, strong) NSPopUpButton *cmbNotify;
@property (nonatomic, strong) NSTextField *lblPort;
@property (nonatomic, strong) NSTextField *editPort;
@property (nonatomic, strong) NSTextField *lblNote;
@property (nonatomic, strong) NSButton *btnClose;
@property (nonatomic, copy) void (^onSettingsChanged)(void);

- (void)applyLanguage;
- (void)applyAppearance;
@end

@implementation SsmSettingsWindowController

- (instancetype)init {
    NSWindow *win = [[NSWindow alloc] initWithContentRect:NSMakeRect(200, 200, 420, 240)
                                                styleMask:NSWindowStyleMaskTitled | NSWindowStyleMaskClosable
                                                  backing:NSBackingStoreBuffered
                                                    defer:NO];
    win.releasedWhenClosed = NO;
    self = [super initWithWindow:win];
    if (self) {
        NSView *cv = [[NSView alloc] initWithFrame:NSMakeRect(0, 0, 420, 240)];
        win.contentView = cv;

        self.lblLang = [NSTextField labelWithString:@""];
        self.lblLang.frame = NSMakeRect(20, 194, 145, 20);
        self.cmbLang = [[NSPopUpButton alloc] initWithFrame:NSMakeRect(172, 190, 226, 26) pullsDown:NO];
        [self.cmbLang addItemsWithTitles:@[@"English", @"日本語"]];
        self.cmbLang.target = self;
        self.cmbLang.action = @selector(onChanged:);

        self.lblTheme = [NSTextField labelWithString:@""];
        self.lblTheme.frame = NSMakeRect(20, 156, 145, 20);
        self.cmbTheme = [[NSPopUpButton alloc] initWithFrame:NSMakeRect(172, 152, 226, 26) pullsDown:NO];
        self.cmbTheme.target = self;
        self.cmbTheme.action = @selector(onChanged:);

        self.lblNotify = [NSTextField labelWithString:@""];
        self.lblNotify.frame = NSMakeRect(20, 118, 145, 20);
        self.cmbNotify = [[NSPopUpButton alloc] initWithFrame:NSMakeRect(172, 114, 226, 26) pullsDown:NO];
        self.cmbNotify.target = self;
        self.cmbNotify.action = @selector(onChanged:);

        self.lblPort = [NSTextField labelWithString:@""];
        self.lblPort.frame = NSMakeRect(20, 80, 145, 20);
        self.editPort = [[NSTextField alloc] initWithFrame:NSMakeRect(172, 78, 110, 24)];
        self.editPort.stringValue = [NSString stringWithFormat:@"%d", gConfiguredApiPort];
        self.editPort.target = self;
        self.editPort.action = @selector(onChanged:);

        self.lblNote = [NSTextField labelWithString:@""];
        self.lblNote.frame = NSMakeRect(20, 50, 380, 18);
        self.lblNote.font = [NSFont systemFontOfSize:11.0];
        self.lblNote.textColor = [NSColor secondaryLabelColor];

        self.btnClose = [[NSButton alloc] initWithFrame:NSMakeRect(318, 14, 80, 28)];
        self.btnClose.bezelStyle = NSBezelStyleRounded;
        self.btnClose.keyEquivalent = @"\r";
        self.btnClose.target = self;
        self.btnClose.action = @selector(onClose:);

        for (NSView *v in @[self.lblLang, self.cmbLang, self.lblTheme, self.cmbTheme,
                            self.lblNotify, self.cmbNotify, self.lblPort, self.editPort,
                            self.lblNote, self.btnClose]) {
            [cv addSubview:v];
        }
        [self applyLanguage];
        [self applyAppearance];
    }
    return self;
}

- (void)applyLanguage {
    self.window.title = TR(@"Settings - Simple Serial Monitor", @"設定 - Simple Serial Monitor");
    self.lblLang.stringValue = TR(@"Language:", @"表示言語:");
    self.lblTheme.stringValue = TR(@"Theme:", @"テーマ:");
    self.lblNotify.stringValue = TR(@"New Device Alert:", @"新デバイス通知:");
    self.lblPort.stringValue = TR(@"Agent API Port:", @"Agent API ポート:");
    self.lblNote.stringValue = TR(@"* API port change takes effect on next launch.", @"※ APIポート変更は次回起動時から反映されます。");
    self.btnClose.title = TR(@"Close", @"閉じる");

    [self.cmbLang selectItemAtIndex:(gIsJa ? 1 : 0)];

    [self.cmbTheme removeAllItems];
    [self.cmbTheme addItemsWithTitles:@[
        TR(@"Follow System", @"システム設定に従う"),
        TR(@"Light", @"ライト"),
        TR(@"Dark", @"ダーク")
    ]];
    NSInteger tIdx = 0;
    if ([gThemeMode isEqualToString:@"light"]) tIdx = 1;
    else if ([gThemeMode isEqualToString:@"dark"]) tIdx = 2;
    [self.cmbTheme selectItemAtIndex:tIdx];

    [self.cmbNotify removeAllItems];
    [self.cmbNotify addItemsWithTitles:@[
        TR(@"Off", @"OFF"),
        TR(@"In-App Popup", @"アプリ内ポップアップ"),
        TR(@"macOS Notification", @"macOS 通知"),
        TR(@"Both", @"両方")
    ]];
    NSInteger nIdx = 1;
    if ([gNotifyMode isEqualToString:@"off"]) nIdx = 0;
    else if ([gNotifyMode isEqualToString:@"windows"]) nIdx = 2;
    else if ([gNotifyMode isEqualToString:@"both"]) nIdx = 3;
    [self.cmbNotify selectItemAtIndex:nIdx];
}

- (void)applyAppearance {
    if ([gThemeMode isEqualToString:@"dark"]) {
        self.window.appearance = [NSAppearance appearanceNamed:NSAppearanceNameDarkAqua];
    } else if ([gThemeMode isEqualToString:@"light"]) {
        self.window.appearance = [NSAppearance appearanceNamed:NSAppearanceNameAqua];
    } else {
        self.window.appearance = nil;
    }
}

- (void)onChanged:(id)sender {
    gIsJa = (self.cmbLang.indexOfSelectedItem == 1);
    ssm_set_lang(gIsJa ? "ja" : "en");

    NSArray *themes = @[@"system", @"light", @"dark"];
    NSInteger tIdx = self.cmbTheme.indexOfSelectedItem;
    if (tIdx >= 0 && tIdx < 3) gThemeMode = themes[tIdx];

    NSArray *notifies = @[@"off", @"in_app", @"windows", @"both"];
    NSInteger nIdx = self.cmbNotify.indexOfSelectedItem;
    if (nIdx >= 0 && nIdx < 4) gNotifyMode = notifies[nIdx];

    int p = self.editPort.intValue;
    if (p >= 1024 && p <= 65535) gConfiguredApiPort = p;

    [self applyLanguage];
    [self applyAppearance];
    if (self.onSettingsChanged) self.onSettingsChanged();
}

- (void)onClose:(id)sender {
    [self onChanged:nil];
    [self.window orderOut:nil];
}

@end

// ============================================================================
// Monitor Window Controller (Primary / Extra / Dedicated)
// ============================================================================

@interface SsmWindowRootView : NSView
@property (nonatomic, weak) SsmMonitorWindowController *controller;
@end

@interface SsmMonitorWindowController : NSWindowController <NSWindowDelegate>
@property (nonatomic, assign) BOOL isPrimary;
@property (nonatomic, assign) BOOL dedicated;
@property (nonatomic, assign) BOOL apiCopiedFlash;

@property (nonatomic, strong) SsmWindowRootView *rootView;
@property (nonatomic, strong) NSButton *btn1x1;
@property (nonatomic, strong) NSButton *btn1x2;
@property (nonatomic, strong) NSButton *btn2x1;
@property (nonatomic, strong) NSButton *btn2x2;
@property (nonatomic, strong) NSButton *btnNewWin;
@property (nonatomic, strong) NSButton *btnRefresh;
@property (nonatomic, strong) NSButton *btnApi;
@property (nonatomic, strong) NSButton *btnOpenLogs;
@property (nonatomic, strong) NSButton *btnSettings;
@property (nonatomic, strong) SsmGridContainerView *gridView;

- (instancetype)initWithStateConfig:(NSDictionary *)wcfg isPrimary:(BOOL)isPrimary;
- (NSDictionary *)exportWindowState;
- (void)layoutRootView;
- (void)refreshPorts:(NSArray<NSDictionary *> *)ports;
- (void)pollTick;
- (void)applyLanguage;
- (void)applyAppearance;
- (void)adjustActivePaneZoom:(NSInteger)delta;
- (void)resetActivePaneZoom;
@end

@implementation SsmWindowRootView
- (BOOL)isFlipped { return YES; }
- (void)layout {
    [super layout];
    [self.controller layoutRootView];
}
@end

// Forward declarations for AppDelegate callbacks
@interface SsmAppDelegate : NSObject <NSApplicationDelegate>
@property (nonatomic, strong) NSMutableArray<SsmMonitorWindowController *> *windows;
@property (nonatomic, strong) SsmSettingsWindowController *settingsWC;
@property (nonatomic, strong) NSTimer *pollTimer;
@property (nonatomic, strong) NSTimer *portTimer;
@property (nonatomic, assign) IONotificationPortRef ioNotifyPort;
@property (nonatomic, assign) io_iterator_t ioAddedIter;
@property (nonatomic, assign) io_iterator_t ioRemovedIter;
@property (nonatomic, copy) NSDictionary *initialConfig;

- (void)saveAllConfig;
- (void)refreshAllPortsAndNotify:(BOOL)notifyNew;
- (void)openExtraWindowWithConfig:(NSDictionary *)wcfg;
- (void)openSettings;
- (void)applyGlobalThemeAndLanguage;
- (void)windowDidClose:(SsmMonitorWindowController *)wc;
@end

@implementation SsmMonitorWindowController

- (NSButton *)makeToolbarButton:(SEL)action {
    NSButton *b = [[NSButton alloc] initWithFrame:NSZeroRect];
    b.bezelStyle = NSBezelStyleRounded;
    b.controlSize = NSControlSizeSmall;
    b.font = [NSFont systemFontOfSize:11.5];
    b.target = self;
    b.action = action;
    return b;
}

- (instancetype)initWithStateConfig:(NSDictionary *)wcfg isPrimary:(BOOL)isPrimary {
    NSDictionary *r = [wcfg[@"rect"] isKindOfClass:[NSDictionary class]] ? wcfg[@"rect"] : @{};
    CGFloat w = r[@"width"] ? MAX(440.0, [r[@"width"] doubleValue]) : (isPrimary ? 1100.0 : 820.0);
    CGFloat h = r[@"height"] ? MAX(320.0, [r[@"height"] doubleValue]) : (isPrimary ? 740.0 : 580.0);
    CGFloat x = r[@"x"] ? [r[@"x"] doubleValue] : 100.0;
    CGFloat y = r[@"y"] ? [r[@"y"] doubleValue] : 100.0;

    // Ensure rect is visible on screen
    NSRect screenVis = [[NSScreen mainScreen] visibleFrame];
    if (x < NSMinX(screenVis) - 100 || x > NSMaxX(screenVis) - 120) x = NSMinX(screenVis) + 80;
    if (y < NSMinY(screenVis) - 100 || y > NSMaxY(screenVis) - 100) y = NSMinY(screenVis) + 80;

    NSWindow *win = [[NSWindow alloc] initWithContentRect:NSMakeRect(x, y, w, h)
                                                styleMask:NSWindowStyleMaskTitled | NSWindowStyleMaskClosable | NSWindowStyleMaskMiniaturizable | NSWindowStyleMaskResizable
                                                  backing:NSBackingStoreBuffered
                                                    defer:NO];
    win.minSize = NSMakeSize(440, 300);
    win.releasedWhenClosed = NO;

    self = [super initWithWindow:win];
    if (self) {
        self.isPrimary = isPrimary;
        self.dedicated = [wcfg[@"dedicated"] boolValue];
        win.delegate = self;

        self.rootView = [[SsmWindowRootView alloc] initWithFrame:NSMakeRect(0, 0, w, h)];
        self.rootView.controller = self;
        self.rootView.wantsLayer = YES;
        win.contentView = self.rootView;

        self.btn1x1 = [self makeToolbarButton:@selector(onLayout1x1:)];
        self.btn1x2 = [self makeToolbarButton:@selector(onLayout1x2:)];
        self.btn2x1 = [self makeToolbarButton:@selector(onLayout2x1:)];
        self.btn2x2 = [self makeToolbarButton:@selector(onLayout2x2:)];
        self.btnNewWin = [self makeToolbarButton:@selector(onNewWindow:)];
        self.btnRefresh = [self makeToolbarButton:@selector(onRefreshPorts:)];
        self.btnApi = [self makeToolbarButton:@selector(onCopyApi:)];
        self.btnOpenLogs = [self makeToolbarButton:@selector(onOpenLogs:)];
        self.btnSettings = [self makeToolbarButton:@selector(onOpenSettings:)];

        for (NSView *v in @[self.btn1x1, self.btn1x2, self.btn2x1, self.btn2x2,
                            self.btnNewWin, self.btnRefresh, self.btnApi, self.btnOpenLogs, self.btnSettings]) {
            [self.rootView addSubview:v];
        }

        if (self.dedicated) {
            self.btn1x1.hidden = YES;
            self.btn1x2.hidden = YES;
            self.btn2x1.hidden = YES;
            self.btn2x2.hidden = YES;
            self.btnNewWin.hidden = YES;
            self.btnApi.hidden = YES;
            self.btnOpenLogs.hidden = YES;
            self.btnSettings.hidden = YES;
        }

        self.gridView = [[SsmGridContainerView alloc] initWithFrame:NSZeroRect];
        self.gridView.layoutMode = self.dedicated ? @"1x1" : ([wcfg[@"layout"] isKindOfClass:[NSString class]] ? wcfg[@"layout"] : @"1x1");
        self.gridView.splitX = wcfg[@"split_x"] ? MIN(0.85, MAX(0.15, [wcfg[@"split_x"] doubleValue])) : 0.5;
        self.gridView.splitY = wcfg[@"split_y"] ? MIN(0.85, MAX(0.15, [wcfg[@"split_y"] doubleValue])) : 0.5;
        self.gridView.onSplitChanged = ^{
            [gAppDelegate saveAllConfig];
        };
        [self.rootView addSubview:self.gridView];

        NSArray *panesCfg = [wcfg[@"panes"] isKindOfClass:[NSArray class]] ? wcfg[@"panes"] : @[];
        for (NSInteger i = 0; i < 4; i++) {
            NSDictionary *pc = (i < (NSInteger)panesCfg.count && [panesCfg[i] isKindOfClass:[NSDictionary class]])
                ? panesCfg[i] : @{};
            SsmPaneView *pane = [[SsmPaneView alloc] initWithConfig:pc dedicated:self.dedicated];
            pane.onConfigChanged = ^{
                [gAppDelegate saveAllConfig];
            };
            pane.onPopoutRequested = ^(NSDictionary *exportedPane) {
                NSDictionary *popCfg = @{
                    @"rect": @{ @"x": @(win.frame.origin.x + 36), @"y": @(win.frame.origin.y - 36), @"width": @680, @"height": @500, @"maximized": @NO },
                    @"layout": @"1x1",
                    @"split_x": @0.5,
                    @"split_y": @0.5,
                    @"dedicated": @YES,
                    @"panes": @[exportedPane]
                };
                [gAppDelegate openExtraWindowWithConfig:popCfg];
            };
            [self.gridView.panes addObject:pane];
            [self.gridView addSubview:pane];
        }

        [self applyLanguage];
        [self applyAppearance];
        [self layoutRootView];

        if ([r[@"maximized"] boolValue]) {
            [win zoom:nil];
        }
    }
    return self;
}

- (NSDictionary *)exportWindowState {
    NSRect fr = self.window.frame;
    NSMutableArray *panesArr = [NSMutableArray array];
    for (SsmPaneView *p in self.gridView.panes) {
        [panesArr addObject:[p exportConfig]];
    }
    return @{
        @"rect": @{
            @"x": @((NSInteger)round(fr.origin.x)),
            @"y": @((NSInteger)round(fr.origin.y)),
            @"width": @((NSInteger)round(fr.size.width)),
            @"height": @((NSInteger)round(fr.size.height)),
            @"maximized": @(self.window.isZoomed)
        },
        @"layout": self.gridView.layoutMode ?: @"1x1",
        @"split_x": @(self.gridView.splitX),
        @"split_y": @(self.gridView.splitY),
        @"dedicated": @(self.dedicated),
        @"panes": panesArr
    };
}

- (void)updateLayoutButtonTitles {
    NSString *lm = self.gridView.layoutMode;
    NSString *t1 = TR(@"1 Pane", @"1画面");
    NSString *t2h = TR(@"2 Horiz", @"2画面横");
    NSString *t2v = TR(@"2 Vert", @"2画面縦");
    NSString *t4 = TR(@"4 Panes", @"4画面");
    self.btn1x1.title = [lm isEqualToString:@"1x1"] ? [NSString stringWithFormat:@"[%@]", t1] : t1;
    self.btn1x2.title = [lm isEqualToString:@"1x2"] ? [NSString stringWithFormat:@"[%@]", t2h] : t2h;
    self.btn2x1.title = [lm isEqualToString:@"2x1"] ? [NSString stringWithFormat:@"[%@]", t2v] : t2v;
    self.btn2x2.title = [lm isEqualToString:@"2x2"] ? [NSString stringWithFormat:@"[%@]", t4] : t4;
}

- (void)applyLanguage {
    if (self.dedicated && self.gridView.panes.count > 0 && self.gridView.panes[0].portName.length > 0) {
        NSString *p = self.gridView.panes[0].portName;
        if ([p hasPrefix:@"/dev/"]) p = [p substringFromIndex:5];
        self.window.title = [NSString stringWithFormat:@"%@ - Simple Serial Monitor", p];
    } else {
        self.window.title = @"Simple Serial Monitor";
    }
    [self updateLayoutButtonTitles];
    self.btnNewWin.title = TR(@"+ Window", @"+ 別ウィンドウ");
    self.btnRefresh.title = TR(@"Refresh Ports", @"ポート再検索");
    self.btnOpenLogs.title = TR(@"Log Folder", @"ログフォルダ");
    self.btnSettings.title = TR(@"Settings", @"設定");
    [self updateApiButtonTitle];

    self.btn1x1.toolTip = TR(@"Single pane view", @"1画面表示");
    self.btn1x2.toolTip = TR(@"2 panes side by side (click again to reset split)", @"2画面横分割（再クリックで均等リセット）");
    self.btn2x1.toolTip = TR(@"2 panes stacked (click again to reset split)", @"2画面縦分割（再クリックで均等リセット）");
    self.btn2x2.toolTip = TR(@"4 panes 2x2 grid (click again to reset split)", @"4画面表示（再クリックで均等リセット）");
    self.btnNewWin.toolTip = TR(@"Open an additional multi-pane window", @"追加のモニタウィンドウを開く");
    self.btnRefresh.toolTip = TR(@"Rescan active serial ports", @"アクティブなシリアルポートを再スキャン");
    self.btnApi.toolTip = TR(@"Click to copy Agent API URL", @"クリックでAgent APIのURLをクリップボードへコピー");
    self.btnOpenLogs.toolTip = TR(@"Open live log folder in Finder", @"リアルタイムログの保存フォルダをFinderで開く");
    self.btnSettings.toolTip = TR(@"Open Settings (⌘,)", @"設定画面を開く (⌘,)");

    for (SsmPaneView *p in self.gridView.panes) {
        [p applyLanguage];
    }
}

- (void)updateApiButtonTitle {
    if (self.apiCopiedFlash) {
        self.btnApi.title = TR(@"Copied URL!", @"URLをコピー!");
    } else if (gActualApiPort > 0) {
        self.btnApi.title = [NSString stringWithFormat:@"Agent API: 127.0.0.1:%d", gActualApiPort];
    } else {
        self.btnApi.title = TR(@"Agent API: Off", @"Agent API: 無効");
    }
}

- (void)applyAppearance {
    if ([gThemeMode isEqualToString:@"dark"]) {
        self.window.appearance = [NSAppearance appearanceNamed:NSAppearanceNameDarkAqua];
    } else if ([gThemeMode isEqualToString:@"light"]) {
        self.window.appearance = [NSAppearance appearanceNamed:NSAppearanceNameAqua];
    } else {
        self.window.appearance = nil;
    }
    BOOL dark = SsmIsDarkActive();
    self.rootView.layer.backgroundColor = (dark ? SsmRGB(24, 27, 34) : SsmRGB(236, 238, 242)).CGColor;
    for (SsmPaneView *p in self.gridView.panes) {
        [p applyAppearance];
    }
    [self.gridView setNeedsDisplay:YES];
}

- (void)layoutRootView {
    CGFloat w = self.rootView.bounds.size.width;
    CGFloat h = self.rootView.bounds.size.height;
    CGFloat topH = self.dedicated ? 30.0 : 32.0;
    CGFloat pad = 6.0;
    CGFloat btnH = 22.0;
    CGFloat by = 5.0;

    if (self.dedicated) {
        self.btnRefresh.frame = NSMakeRect(pad, by, 100.0, btnH);
    } else {
        CGFloat x = pad;
        self.btn1x1.frame = NSMakeRect(x, by, 64.0, btnH); x += 68.0;
        self.btn1x2.frame = NSMakeRect(x, by, 68.0, btnH); x += 72.0;
        self.btn2x1.frame = NSMakeRect(x, by, 68.0, btnH); x += 72.0;
        self.btn2x2.frame = NSMakeRect(x, by, 64.0, btnH); x += 72.0;
        self.btnNewWin.frame = NSMakeRect(x, by, 104.0, btnH); x += 108.0;
        self.btnRefresh.frame = NSMakeRect(x, by, 96.0, btnH);

        CGFloat setW = 60.0;
        CGFloat logW = 90.0;
        CGFloat apiW = 166.0;
        CGFloat rx = w - pad - setW;
        self.btnSettings.frame = NSMakeRect(rx, by, setW, btnH);
        rx -= (logW + 4.0);
        self.btnOpenLogs.frame = NSMakeRect(rx, by, logW, btnH);
        rx -= (apiW + 4.0);
        self.btnApi.frame = NSMakeRect(rx, by, apiW, btnH);
        self.btnApi.hidden = (rx < x + 102.0);
    }

    self.gridView.frame = NSMakeRect(pad, topH, MAX(100.0, w - pad * 2), MAX(100.0, h - topH - pad));
    self.gridView.needsLayout = YES;
}

- (void)setLayoutMode:(NSString *)mode {
    if ([self.gridView.layoutMode isEqualToString:mode]) {
        self.gridView.splitX = 0.5;
        self.gridView.splitY = 0.5;
    } else {
        self.gridView.layoutMode = mode;
    }
    [self updateLayoutButtonTitles];
    self.gridView.needsLayout = YES;
    [self.gridView layoutSubtreeIfNeeded];
    [gAppDelegate saveAllConfig];
}

- (void)onLayout1x1:(id)sender { [self setLayoutMode:@"1x1"]; }
- (void)onLayout1x2:(id)sender { [self setLayoutMode:@"1x2"]; }
- (void)onLayout2x1:(id)sender { [self setLayoutMode:@"2x1"]; }
- (void)onLayout2x2:(id)sender { [self setLayoutMode:@"2x2"]; }

- (void)onNewWindow:(id)sender {
    NSRect fr = self.window.frame;
    NSDictionary *wcfg = @{
        @"rect": @{ @"x": @(fr.origin.x + 32), @"y": @(fr.origin.y - 32), @"width": @860, @"height": @620, @"maximized": @NO },
        @"layout": @"1x1",
        @"split_x": @0.5,
        @"split_y": @0.5,
        @"dedicated": @NO,
        @"panes": @[@{}]
    };
    [gAppDelegate openExtraWindowWithConfig:wcfg];
}

- (void)onRefreshPorts:(id)sender {
    [gAppDelegate refreshAllPortsAndNotify:NO];
}

- (void)onCopyApi:(id)sender {
    if (gActualApiPort <= 0) return;
    NSString *url = [NSString stringWithFormat:@"http://127.0.0.1:%d", gActualApiPort];
    NSPasteboard *pb = [NSPasteboard generalPasteboard];
    [pb clearContents];
    [pb setString:url forType:NSPasteboardTypeString];
    self.apiCopiedFlash = YES;
    [self updateApiButtonTitle];
    __weak typeof(self) weakSelf = self;
    dispatch_after(dispatch_time(DISPATCH_TIME_NOW, (int64_t)(1.5 * NSEC_PER_SEC)), dispatch_get_main_queue(), ^{
        weakSelf.apiCopiedFlash = NO;
        [weakSelf updateApiButtonTitle];
    });
}

- (void)onOpenLogs:(id)sender {
    char *cdir = ssm_get_log_dir();
    if (!cdir) return;
    NSString *dir = [NSString stringWithUTF8String:cdir];
    ssm_free_str(cdir);
    if (dir.length > 0) {
        [[NSWorkspace sharedWorkspace] openURL:[NSURL fileURLWithPath:dir]];
    }
}

- (void)onOpenSettings:(id)sender {
    [gAppDelegate openSettings];
}

- (void)refreshPorts:(NSArray<NSDictionary *> *)ports {
    for (SsmPaneView *p in self.gridView.panes) {
        [p refreshPortsList:ports];
    }
}

- (void)pollTick {
    NSInteger vis = self.gridView.panes.count;
    for (NSInteger i = 0; i < vis; i++) {
        SsmPaneView *p = self.gridView.panes[i];
        if (!p.hidden) {
            [p pollTick];
        }
    }
}

- (SsmPaneView *)activePane {
    NSResponder *fr = self.window.firstResponder;
    if ([fr isKindOfClass:[NSView class]]) {
        NSView *v = (NSView *)fr;
        while (v) {
            if ([v isKindOfClass:[SsmPaneView class]]) return (SsmPaneView *)v;
            v = v.superview;
        }
    }
    return self.gridView.panes.firstObject;
}

- (void)adjustActivePaneZoom:(NSInteger)delta {
    [[self activePane].logView adjustZoom:delta];
}

- (void)resetActivePaneZoom {
    [[self activePane].logView resetZoom];
}

- (void)windowDidMove:(NSNotification *)notification {
    if (!gShuttingDown) [gAppDelegate saveAllConfig];
}

- (void)windowDidResize:(NSNotification *)notification {
    if (!gShuttingDown) [gAppDelegate saveAllConfig];
}

- (void)windowWillClose:(NSNotification *)notification {
    for (SsmPaneView *p in self.gridView.panes) {
        [p cleanup];
    }
    [gAppDelegate windowDidClose:self];
}

@end

// ============================================================================
// IOKit Hotplug Callback
// ============================================================================

static void SsmIoKitDeviceCallback(void *refcon, io_iterator_t iterator) {
    io_object_t obj;
    BOOL any = NO;
    while ((obj = IOIteratorNext(iterator)) != 0) {
        IOObjectRelease(obj);
        any = YES;
    }
    if (any && gAppDelegate && !gShuttingDown) {
        dispatch_after(dispatch_time(DISPATCH_TIME_NOW, (int64_t)(250 * NSEC_PER_MSEC)), dispatch_get_main_queue(), ^{
            [gAppDelegate refreshAllPortsAndNotify:YES];
        });
    }
}

// ============================================================================
// Application Delegate
// ============================================================================

@implementation SsmAppDelegate

- (instancetype)init {
    self = [super init];
    if (self) {
        self.windows = [NSMutableArray array];
    }
    return self;
}

- (void)setupMenuBar {
    NSMenu *mainMenu = [[NSMenu alloc] init];

    // App menu
    NSMenuItem *appItem = [[NSMenuItem alloc] init];
    [mainMenu addItem:appItem];
    NSMenu *appMenu = [[NSMenu alloc] init];
    [appMenu addItemWithTitle:TR(@"Settings...", @"設定...") action:@selector(menuOpenSettings:) keyEquivalent:@","];
    [appMenu addItem:[NSMenuItem separatorItem]];
    [appMenu addItemWithTitle:TR(@"Quit Simple Serial Monitor", @"Simple Serial Monitor を終了") action:@selector(terminate:) keyEquivalent:@"q"];
    appItem.submenu = appMenu;

    // File menu
    NSMenuItem *fileItem = [[NSMenuItem alloc] init];
    [mainMenu addItem:fileItem];
    NSMenu *fileMenu = [[NSMenu alloc] initWithTitle:TR(@"File", @"ファイル")];
    [fileMenu addItemWithTitle:TR(@"New Window", @"新規ウィンドウ") action:@selector(menuNewWindow:) keyEquivalent:@"n"];
    [fileMenu addItemWithTitle:TR(@"Close Window", @"ウィンドウを閉じる") action:@selector(performClose:) keyEquivalent:@"w"];
    fileItem.submenu = fileMenu;

    // Edit menu
    NSMenuItem *editItem = [[NSMenuItem alloc] init];
    [mainMenu addItem:editItem];
    NSMenu *editMenu = [[NSMenu alloc] initWithTitle:TR(@"Edit", @"編集")];
    [editMenu addItemWithTitle:TR(@"Cut", @"切り取り") action:@selector(cut:) keyEquivalent:@"x"];
    [editMenu addItemWithTitle:TR(@"Copy", @"コピー") action:@selector(copy:) keyEquivalent:@"c"];
    [editMenu addItemWithTitle:TR(@"Paste", @"ペースト") action:@selector(paste:) keyEquivalent:@"v"];
    [editMenu addItemWithTitle:TR(@"Select All", @"すべて選択") action:@selector(selectAll:) keyEquivalent:@"a"];
    editItem.submenu = editMenu;

    // View menu
    NSMenuItem *viewItem = [[NSMenuItem alloc] init];
    [mainMenu addItem:viewItem];
    NSMenu *viewMenu = [[NSMenu alloc] initWithTitle:TR(@"View", @"表示")];
    [viewMenu addItemWithTitle:TR(@"Zoom In", @"拡大") action:@selector(menuZoomIn:) keyEquivalent:@"+"];
    [viewMenu addItemWithTitle:TR(@"Zoom Out", @"縮小") action:@selector(menuZoomOut:) keyEquivalent:@"-"];
    [viewMenu addItemWithTitle:TR(@"Reset Zoom", @"ズームをリセット") action:@selector(menuZoomReset:) keyEquivalent:@"0"];
    viewItem.submenu = viewMenu;

    NSApp.mainMenu = mainMenu;
}

- (void)setupIoKitHotplug {
    self.ioNotifyPort = IONotificationPortCreate(kIOMasterPortDefault);
    if (!self.ioNotifyPort) return;
    CFRunLoopSourceRef src = IONotificationPortGetRunLoopSource(self.ioNotifyPort);
    CFRunLoopAddSource(CFRunLoopGetMain(), src, kCFRunLoopDefaultMode);

    CFMutableDictionaryRef matchAdd = IOServiceMatching(kIOSerialBSDServiceValue);
    if (matchAdd) {
        if (IOServiceAddMatchingNotification(self.ioNotifyPort, kIOMatchedNotification, matchAdd,
                                             SsmIoKitDeviceCallback, NULL, &_ioAddedIter) == KERN_SUCCESS) {
            io_object_t obj;
            while ((obj = IOIteratorNext(self.ioAddedIter)) != 0) IOObjectRelease(obj);
        }
    }
    CFMutableDictionaryRef matchRem = IOServiceMatching(kIOSerialBSDServiceValue);
    if (matchRem) {
        if (IOServiceAddMatchingNotification(self.ioNotifyPort, kIOTerminatedNotification, matchRem,
                                             SsmIoKitDeviceCallback, NULL, &_ioRemovedIter) == KERN_SUCCESS) {
            io_object_t obj;
            while ((obj = IOIteratorNext(self.ioRemovedIter)) != 0) IOObjectRelease(obj);
        }
    }
}

- (void)applicationDidFinishLaunching:(NSNotification *)notification {
    NSDictionary *cfg = self.initialConfig ?: @{};
    gIsJa = [[cfg[@"lang"] isKindOfClass:[NSString class]] ? cfg[@"lang"] : @"en" isEqualToString:@"ja"];
    gThemeMode = [cfg[@"theme"] isKindOfClass:[NSString class]] ? cfg[@"theme"] : @"system";
    gNotifyMode = [cfg[@"notify_mode"] isKindOfClass:[NSString class]] ? cfg[@"notify_mode"] : @"in_app";
    gConfiguredApiPort = cfg[@"api_port"] ? [cfg[@"api_port"] intValue] : 27182;

    [self setupMenuBar];

    // Fetch initial ports
    NSDictionary *portsResp = SsmParseJsonC(ssm_get_ports_json(0));
    NSArray *ports = [portsResp[@"ports"] isKindOfClass:[NSArray class]] ? portsResp[@"ports"] : @[];

    // Build main window config (auto-select first USB serial port if none saved)
    NSMutableArray *mainPanes = [NSMutableArray array];
    NSArray *savedMainPanes = [cfg[@"main_panes"] isKindOfClass:[NSArray class]] ? cfg[@"main_panes"] : @[];
    for (NSDictionary *p in savedMainPanes) {
        [mainPanes addObject:[p mutableCopy]];
    }
    while (mainPanes.count < 4) [mainPanes addObject:[NSMutableDictionary dictionary]];

    NSArray *extraWins = [cfg[@"extra_windows"] isKindOfClass:[NSArray class]] ? cfg[@"extra_windows"] : @[];
    if (([mainPanes[0][@"port"] length] == 0) && extraWins.count == 0 && ports.count > 0) {
        NSString *chosen = ports[0][@"port"];
        if (chosen.length > 0) {
            mainPanes[0][@"port"] = chosen;
            mainPanes[0][@"connected"] = @YES;
        }
    }

    NSDictionary *mainWcfg = @{
        @"rect": [cfg[@"main_window"] isKindOfClass:[NSDictionary class]] ? cfg[@"main_window"] : @{},
        @"layout": [cfg[@"layout"] isKindOfClass:[NSString class]] ? cfg[@"layout"] : @"1x1",
        @"split_x": cfg[@"main_split_x"] ?: @0.5,
        @"split_y": cfg[@"main_split_y"] ?: @0.5,
        @"dedicated": @NO,
        @"panes": mainPanes
    };

    SsmMonitorWindowController *mainWC = [[SsmMonitorWindowController alloc] initWithStateConfig:mainWcfg isPrimary:YES];
    [mainWC refreshPorts:ports];
    [self.windows addObject:mainWC];
    [mainWC showWindow:nil];

    for (NSDictionary *ew in extraWins) {
        if ([ew isKindOfClass:[NSDictionary class]]) {
            SsmMonitorWindowController *ewc = [[SsmMonitorWindowController alloc] initWithStateConfig:ew isPrimary:NO];
            [ewc refreshPorts:ports];
            [self.windows addObject:ewc];
            [ewc showWindow:nil];
        }
    }

    [self setupIoKitHotplug];

    self.pollTimer = [NSTimer scheduledTimerWithTimeInterval:0.05
                                                      target:self
                                                    selector:@selector(onPollTimer:)
                                                    userInfo:nil
                                                     repeats:YES];
    self.portTimer = [NSTimer scheduledTimerWithTimeInterval:2.5
                                                      target:self
                                                    selector:@selector(onPortTimer:)
                                                    userInfo:nil
                                                     repeats:YES];

    [NSApp activateIgnoringOtherApps:YES];
}

- (void)onPollTimer:(NSTimer *)timer {
    for (SsmMonitorWindowController *wc in [self.windows copy]) {
        [wc pollTick];
    }
}

- (void)onPortTimer:(NSTimer *)timer {
    [self refreshAllPortsAndNotify:YES];
}

- (void)refreshAllPortsAndNotify:(BOOL)notifyNew {
    NSDictionary *resp = SsmParseJsonC(ssm_get_ports_json(1));
    if (![resp isKindOfClass:[NSDictionary class]]) return;
    NSArray *ports = [resp[@"ports"] isKindOfClass:[NSArray class]] ? resp[@"ports"] : @[];
    NSArray *added = [resp[@"added"] isKindOfClass:[NSArray class]] ? resp[@"added"] : @[];

    for (SsmMonitorWindowController *wc in self.windows) {
        [wc refreshPorts:ports];
    }

    if (notifyNew && added.count > 0 && ![gNotifyMode isEqualToString:@"off"]) {
        NSString *title = added.count == 1
            ? TR(@"New serial device detected", @"新しいシリアルデバイスを検出")
            : (gIsJa
                ? [NSString stringWithFormat:@"新しいシリアルデバイスを %lu 件検出", (unsigned long)added.count]
                : [NSString stringWithFormat:@"%lu new serial devices detected", (unsigned long)added.count]);
        NSMutableArray<NSString *> *labels = [NSMutableArray array];
        for (NSDictionary *p in added) {
            [labels addObject:SsmShortPortLabel(p[@"port"] ?: @"", p[@"hardware_name"] ?: @"")];
        }
        NSString *body = [labels componentsJoinedByString:@"\n"];

        if ([gNotifyMode isEqualToString:@"in_app"] || [gNotifyMode isEqualToString:@"both"]) {
            NSWindow *owner = self.windows.firstObject.window;
            [SsmToastPanel showNearWindow:owner title:title body:body];
        }
        if ([gNotifyMode isEqualToString:@"windows"] || [gNotifyMode isEqualToString:@"both"]) {
            NSString *escTitle = [title stringByReplacingOccurrencesOfString:@"\"" withString:@"\\\""];
            NSString *escBody = [[body stringByReplacingOccurrencesOfString:@"\n" withString:@", "]
                                 stringByReplacingOccurrencesOfString:@"\"" withString:@"\\\""];
            NSString *script = [NSString stringWithFormat:@"display notification \"%@\" with title \"%@\"", escBody, escTitle];
            dispatch_async(dispatch_get_global_queue(DISPATCH_QUEUE_PRIORITY_LOW, 0), ^{
                NSTask *task = [[NSTask alloc] init];
                task.executableURL = [NSURL fileURLWithPath:@"/usr/bin/osascript"];
                task.arguments = @[@"-e", script];
                [task launchAndReturnError:nil];
            });
        }
    }
}

- (void)openExtraWindowWithConfig:(NSDictionary *)wcfg {
    NSDictionary *resp = SsmParseJsonC(ssm_get_ports_json(0));
    NSArray *ports = [resp[@"ports"] isKindOfClass:[NSArray class]] ? resp[@"ports"] : @[];
    SsmMonitorWindowController *wc = [[SsmMonitorWindowController alloc] initWithStateConfig:wcfg isPrimary:NO];
    [wc refreshPorts:ports];
    [self.windows addObject:wc];
    [wc showWindow:nil];
    [self saveAllConfig];
}

- (void)openSettings {
    if (!self.settingsWC) {
        self.settingsWC = [[SsmSettingsWindowController alloc] init];
        __weak typeof(self) weakSelf = self;
        self.settingsWC.onSettingsChanged = ^{
            [weakSelf applyGlobalThemeAndLanguage];
            [weakSelf saveAllConfig];
        };
    }
    [self.settingsWC applyLanguage];
    [self.settingsWC applyAppearance];
    [self.settingsWC.window center];
    [self.settingsWC showWindow:nil];
}

- (void)applyGlobalThemeAndLanguage {
    [self setupMenuBar];
    for (SsmMonitorWindowController *wc in self.windows) {
        [wc applyLanguage];
        [wc applyAppearance];
    }
}

- (void)saveAllConfig {
    if (self.windows.count == 0) return;
    SsmMonitorWindowController *primary = nil;
    NSMutableArray *extras = [NSMutableArray array];
    for (SsmMonitorWindowController *wc in self.windows) {
        if (wc.isPrimary && !primary) {
            primary = wc;
        } else {
            [extras addObject:[wc exportWindowState]];
        }
    }
    if (!primary) primary = self.windows.firstObject;
    NSDictionary *mainState = [primary exportWindowState];

    NSDictionary *fullCfg = @{
        @"version": @3,
        @"api_port": @(gConfiguredApiPort),
        @"lang": gIsJa ? @"ja" : @"en",
        @"theme": gThemeMode ?: @"system",
        @"notify_mode": gNotifyMode ?: @"in_app",
        @"main_window": mainState[@"rect"] ?: @{},
        @"layout": mainState[@"layout"] ?: @"1x1",
        @"main_split_x": mainState[@"split_x"] ?: @0.5,
        @"main_split_y": mainState[@"split_y"] ?: @0.5,
        @"main_panes": mainState[@"panes"] ?: @[],
        @"extra_windows": extras
    };
    ssm_save_config_json(SsmToJsonString(fullCfg).UTF8String);
}

- (void)windowDidClose:(SsmMonitorWindowController *)wc {
    if (!gShuttingDown) {
        [self saveAllConfig];
    }
    [self.windows removeObject:wc];
    if (wc.isPrimary || self.windows.count == 0) {
        gShuttingDown = YES;
        [NSApp terminate:nil];
    }
}

- (void)applicationWillTerminate:(NSNotification *)notification {
    gShuttingDown = YES;
    [self saveAllConfig];
    [self.pollTimer invalidate];
    [self.portTimer invalidate];
    if (self.ioAddedIter) IOObjectRelease(self.ioAddedIter);
    if (self.ioRemovedIter) IOObjectRelease(self.ioRemovedIter);
    if (self.ioNotifyPort) IONotificationPortDestroy(self.ioNotifyPort);
    ssm_shutdown_all();
}

- (BOOL)applicationShouldTerminateAfterLastWindowClosed:(NSApplication *)sender {
    return YES;
}

// Menu actions
- (void)menuOpenSettings:(id)sender { [self openSettings]; }
- (void)menuNewWindow:(id)sender {
    SsmMonitorWindowController *wc = self.windows.firstObject;
    if (wc) [wc onNewWindow:nil];
}
- (void)menuZoomIn:(id)sender {
    for (SsmMonitorWindowController *wc in self.windows) {
        if (wc.window.isKeyWindow) { [wc adjustActivePaneZoom:10]; return; }
    }
}
- (void)menuZoomOut:(id)sender {
    for (SsmMonitorWindowController *wc in self.windows) {
        if (wc.window.isKeyWindow) { [wc adjustActivePaneZoom:-10]; return; }
    }
}
- (void)menuZoomReset:(id)sender {
    for (SsmMonitorWindowController *wc in self.windows) {
        if (wc.window.isKeyWindow) { [wc resetActivePaneZoom]; return; }
    }
}

@end

// ============================================================================
// C Entry Point
// ============================================================================

void ssm_macos_run_gui(const char *initial_config_json, int actual_api_port) {
    @autoreleasepool {
        gActualApiPort = actual_api_port;
        NSApplication *app = [NSApplication sharedApplication];
        [app setActivationPolicy:NSApplicationActivationPolicyRegular];

        gAppDelegate = [[SsmAppDelegate alloc] init];
        if (initial_config_json) {
            NSData *data = [NSData dataWithBytes:initial_config_json length:strlen(initial_config_json)];
            id parsed = [NSJSONSerialization JSONObjectWithData:data options:0 error:nil];
            if ([parsed isKindOfClass:[NSDictionary class]]) {
                gAppDelegate.initialConfig = parsed;
            }
        }
        app.delegate = gAppDelegate;
        [app run];
    }
}
