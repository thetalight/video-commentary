#import <AVFoundation/AVFoundation.h>
#import <CoreGraphics/CoreGraphics.h>
#import <Foundation/Foundation.h>
#import <Vision/Vision.h>

@interface VCCaption : NSObject
@property(nonatomic) double start;
@property(nonatomic) double end;
@property(nonatomic, copy) NSString *text;
@property(nonatomic) float confidence;
@end
@implementation VCCaption
@end

static void VCProgress(double progress, NSString *message) {
    NSDictionary *payload = @{
        @"progress" : @(MAX(0, MIN(100, progress))),
        @"message" : message ?: @""
    };
    NSData *data = [NSJSONSerialization dataWithJSONObject:payload options:0 error:nil];
    if (data) {
        fwrite(data.bytes, 1, data.length, stderr);
        fwrite("\n", 1, 1, stderr);
        fflush(stderr);
    }
}

static NSString *VCNormalized(NSString *value) {
    NSString *lower = value.lowercaseString ?: @"";
    NSMutableString *result = [NSMutableString string];
    NSCharacterSet *alphanumeric = NSCharacterSet.alphanumericCharacterSet;
    for (NSUInteger index = 0; index < lower.length; index++) {
        unichar character = [lower characterAtIndex:index];
        BOOL cjk = (character >= 0x3040 && character <= 0x30ff) ||
                   (character >= 0x3400 && character <= 0x9fff) ||
                   (character >= 0xac00 && character <= 0xd7af);
        if (cjk || [alphanumeric characterIsMember:character]) {
            [result appendFormat:@"%C", character];
        }
    }
    return result;
}

static NSUInteger VCEditDistance(NSString *left, NSString *right) {
    NSUInteger rows = left.length + 1;
    NSUInteger columns = right.length + 1;
    NSUInteger *previous = calloc(columns, sizeof(NSUInteger));
    NSUInteger *current = calloc(columns, sizeof(NSUInteger));
    for (NSUInteger column = 0; column < columns; column++) previous[column] = column;
    for (NSUInteger row = 1; row < rows; row++) {
        current[0] = row;
        unichar a = [left characterAtIndex:row - 1];
        for (NSUInteger column = 1; column < columns; column++) {
            unichar b = [right characterAtIndex:column - 1];
            NSUInteger insertion = current[column - 1] + 1;
            NSUInteger deletion = previous[column] + 1;
            NSUInteger replace = previous[column - 1] + (a == b ? 0 : 1);
            current[column] = MIN(insertion, MIN(deletion, replace));
        }
        NSUInteger *swap = previous;
        previous = current;
        current = swap;
    }
    NSUInteger result = previous[columns - 1];
    free(previous);
    free(current);
    return result;
}

static BOOL VCSimilar(NSString *left, NSString *right) {
    NSString *a = VCNormalized(left);
    NSString *b = VCNormalized(right);
    if (a.length == 0 || b.length == 0) return NO;
    if ([a isEqualToString:b]) return YES;
    NSUInteger longest = MAX(a.length, b.length);
    if (longest <= 3) return NO;
    return (double)VCEditDistance(a, b) / (double)longest <= 0.18;
}

static BOOL VCHasText(NSString *value) { return VCNormalized(value).length >= 2; }

static CGImageRef VCCopyFrame(AVAssetImageGenerator *generator, double second) {
    CMTime actual = kCMTimeZero;
    CMTime requested = CMTimeMakeWithSeconds(second, 600);
    return [generator copyCGImageAtTime:requested actualTime:&actual error:nil];
}

static NSDictionary *VCRecognize(CGImageRef image, VNRecognizeTextRequest *request) {
    VNImageRequestHandler *handler = [[VNImageRequestHandler alloc] initWithCGImage:image options:@{}];
    if (![handler performRequests:@[ request ] error:nil]) return @{ @"text" : @"", @"confidence" : @0 };
    NSArray<VNRecognizedTextObservation *> *observations = request.results ?: @[];
    observations = [observations sortedArrayUsingComparator:^NSComparisonResult(
        VNRecognizedTextObservation *left, VNRecognizedTextObservation *right) {
        if (fabs(CGRectGetMidY(left.boundingBox) - CGRectGetMidY(right.boundingBox)) > 0.025) {
            return CGRectGetMidY(left.boundingBox) > CGRectGetMidY(right.boundingBox)
                       ? NSOrderedAscending
                       : NSOrderedDescending;
        }
        return CGRectGetMinX(left.boundingBox) < CGRectGetMinX(right.boundingBox)
                   ? NSOrderedAscending
                   : NSOrderedDescending;
    }];
    NSMutableArray<NSString *> *lines = [NSMutableArray array];
    float confidence = 0;
    for (VNRecognizedTextObservation *observation in observations) {
        VNRecognizedText *candidate = [observation topCandidates:1].firstObject;
        if (!candidate || candidate.confidence < 0.35 || !VCHasText(candidate.string)) continue;
        [lines addObject:candidate.string];
        confidence += candidate.confidence;
    }
    if (lines.count == 0) return @{ @"text" : @"", @"confidence" : @0 };
    NSString *text = [[lines componentsJoinedByString:@" "]
        stringByTrimmingCharactersInSet:NSCharacterSet.whitespaceAndNewlineCharacterSet];
    return @{ @"text" : text, @"confidence" : @(confidence / lines.count) };
}

static BOOL VCProbe(double duration, AVAssetImageGenerator *generator, VNRecognizeTextRequest *request) {
    NSInteger count = duration >= 1200 ? 32 : 20;
    NSInteger hits = 0;
    NSMutableSet<NSString *> *unique = [NSMutableSet set];
    for (NSInteger index = 0; index < count; index++) {
        @autoreleasepool {
            double second = duration * ((double)index + 0.5) / (double)count;
            CGImageRef frame = VCCopyFrame(generator, second);
            if (!frame) continue;
            NSDictionary *recognized = VCRecognize(frame, request);
            CGImageRelease(frame);
            NSString *key = VCNormalized(recognized[@"text"]);
            if (key.length >= 2 && [recognized[@"confidence"] floatValue] >= 0.42) {
                hits++;
                [unique addObject:key];
            }
        }
    }
    return hits >= 4 && unique.count >= 3;
}

static NSArray<VCCaption *> *VCRemoveStaticOverlays(NSArray<VCCaption *> *captions) {
    NSMutableDictionary<NSString *, NSMutableArray<NSNumber *> *> *occurrences = [NSMutableDictionary dictionary];
    [captions enumerateObjectsUsingBlock:^(VCCaption *caption, NSUInteger index, BOOL *stop) {
        NSString *key = VCNormalized(caption.text);
        if (key.length >= 4) {
            if (!occurrences[key]) occurrences[key] = [NSMutableArray array];
            [occurrences[key] addObject:@(index)];
        }
    }];
    NSMutableIndexSet *rejected = [NSMutableIndexSet indexSet];
    for (NSArray<NSNumber *> *indices in occurrences.allValues) {
        if (indices.count < 8) continue;
        VCCaption *first = captions[indices.firstObject.unsignedIntegerValue];
        VCCaption *last = captions[indices.lastObject.unsignedIntegerValue];
        if (last.start - first.start < 60) continue;
        for (NSNumber *index in indices) [rejected addIndex:index.unsignedIntegerValue];
    }
    NSMutableArray<VCCaption *> *result = [NSMutableArray array];
    [captions enumerateObjectsUsingBlock:^(VCCaption *caption, NSUInteger index, BOOL *stop) {
        if (![rejected containsIndex:index]) [result addObject:caption];
    }];
    return result;
}

static NSString *VCTime(double seconds) {
    NSInteger total = (NSInteger)llround(MAX(0, seconds) * 1000);
    NSInteger milliseconds = total % 1000;
    NSInteger allSeconds = total / 1000;
    NSInteger second = allSeconds % 60;
    NSInteger allMinutes = allSeconds / 60;
    return [NSString stringWithFormat:@"%02ld:%02ld:%02ld,%03ld",
                                      (long)(allMinutes / 60), (long)(allMinutes % 60),
                                      (long)second, (long)milliseconds];
}

static BOOL VCWriteSRT(NSArray<VCCaption *> *captions, NSString *path) {
    NSMutableString *result = [NSMutableString string];
    [captions enumerateObjectsUsingBlock:^(VCCaption *caption, NSUInteger index, BOOL *stop) {
        [result appendFormat:@"%lu\n%@ --> %@\n%@\n\n", (unsigned long)index + 1,
                             VCTime(caption.start), VCTime(caption.end), caption.text];
    }];
    [[NSFileManager defaultManager] createDirectoryAtPath:path.stringByDeletingLastPathComponent
                              withIntermediateDirectories:YES attributes:nil error:nil];
    return [result writeToFile:path atomically:YES encoding:NSUTF8StringEncoding error:nil];
}

int main(int argc, const char *argv[]) {
    @autoreleasepool {
        if (argc < 3) {
            fprintf(stderr, "usage: subtitle-ocr <video> <output.srt> [interval]\n");
            return 2;
        }
        NSString *videoPath = [NSString stringWithUTF8String:argv[1]];
        NSString *outputPath = [NSString stringWithUTF8String:argv[2]];
        double interval = argc > 3 ? atof(argv[3]) : 0.75;
        interval = MAX(0.5, interval);
        AVURLAsset *asset = [AVURLAsset URLAssetWithURL:[NSURL fileURLWithPath:videoPath] options:nil];
        double duration = CMTimeGetSeconds(asset.duration);
        if (!isfinite(duration) || duration <= 1) return 2;

        AVAssetImageGenerator *generator = [AVAssetImageGenerator assetImageGeneratorWithAsset:asset];
        generator.appliesPreferredTrackTransform = YES;
        generator.maximumSize = CGSizeMake(1280, 720);
        generator.requestedTimeToleranceBefore = CMTimeMakeWithSeconds(0.12, 600);
        generator.requestedTimeToleranceAfter = CMTimeMakeWithSeconds(0.12, 600);

        VNRecognizeTextRequest *request = [[VNRecognizeTextRequest alloc] init];
        request.recognitionLevel = VNRequestTextRecognitionLevelAccurate;
        request.usesLanguageCorrection = YES;
        request.minimumTextHeight = 0.018;
        request.regionOfInterest = CGRectMake(0.04, 0.0, 0.92, 0.48);
        NSArray<NSString *> *supported = [VNRecognizeTextRequest supportedRecognitionLanguagesForTextRecognitionLevel:VNRequestTextRecognitionLevelAccurate revision:request.revision error:nil];
        NSArray<NSString *> *preferred = @[ @"zh-Hans", @"zh-Hant", @"ja-JP", @"ko-KR", @"en-US" ];
        NSMutableArray<NSString *> *selected = [NSMutableArray array];
        for (NSString *language in preferred) if ([supported containsObject:language]) [selected addObject:language];
        if (selected.count) request.recognitionLanguages = selected;

        VCProgress(1, @"正在检查画面是否包含硬字幕...");
        if (!VCProbe(duration, generator, request)) {
            VCProgress(100, @"未检测到稳定的画面硬字幕");
            return 3;
        }

        NSInteger total = (NSInteger)ceil(duration / interval);
        NSMutableArray<VCCaption *> *captions = [NSMutableArray array];
        VCCaption *active = nil;
        double lastSeen = 0;
        for (NSInteger index = 0; index < total; index++) {
            @autoreleasepool {
                double second = MIN(duration - 0.01, index * interval);
                CGImageRef frame = VCCopyFrame(generator, second);
                NSDictionary *recognized = frame ? VCRecognize(frame, request) : @{ @"text" : @"", @"confidence" : @0 };
                if (frame) CGImageRelease(frame);
                NSString *text = recognized[@"text"];
                float confidence = [recognized[@"confidence"] floatValue];
                if (VCHasText(text) && confidence >= 0.38) {
                    if (active && VCSimilar(active.text, text)) {
                        if (text.length > active.text.length || confidence > active.confidence + 0.08) active.text = text;
                        active.confidence = MAX(active.confidence, confidence);
                        active.end = MIN(duration, second + interval);
                        lastSeen = second;
                    } else {
                        if (active) {
                            active.end = MAX(active.start + 0.4, MIN(active.end, second));
                            [captions addObject:active];
                        }
                        active = [VCCaption new];
                        active.start = second;
                        active.end = MIN(duration, second + interval);
                        active.text = text;
                        active.confidence = confidence;
                        lastSeen = second;
                    }
                } else if (active && second - lastSeen >= interval) {
                    active.end = MAX(active.start + 0.4, MIN(duration, lastSeen + interval));
                    [captions addObject:active];
                    active = nil;
                }
                if (index % 20 == 0) {
                    VCProgress(5 + 92.0 * index / MAX(total, 1),
                               [NSString stringWithFormat:@"正在本地识别画面字幕 %ld/%ld", (long)index, (long)total]);
                }
            }
        }
        if (active) [captions addObject:active];
        NSArray<VCCaption *> *filtered = [VCRemoveStaticOverlays(captions) filteredArrayUsingPredicate:[NSPredicate predicateWithBlock:^BOOL(VCCaption *caption, NSDictionary *bindings) {
            return VCHasText(caption.text) && caption.end - caption.start >= 0.35;
        }]];
        NSInteger minimum = duration >= 1200 ? 30 : MAX(5, (NSInteger)(duration / 240));
        NSMutableIndexSet *quarters = [NSMutableIndexSet indexSet];
        for (VCCaption *caption in filtered) [quarters addIndex:MIN(3, (NSUInteger)((caption.start / duration) * 4))];
        if (filtered.count < minimum || (duration >= 1200 && quarters.count < 3)) {
            VCProgress(100, @"画面文字不足以构成连续字幕，已放弃 OCR 结果");
            return 3;
        }
        if (!VCWriteSRT(filtered, outputPath)) return 2;
        VCProgress(100, [NSString stringWithFormat:@"画面硬字幕识别完成，共 %lu 条", (unsigned long)filtered.count]);
        return 0;
    }
}
