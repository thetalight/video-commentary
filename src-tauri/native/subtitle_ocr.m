#import <AVFoundation/AVFoundation.h>
#import <CoreGraphics/CoreGraphics.h>
#import <Foundation/Foundation.h>
#import <Vision/Vision.h>

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

static BOOL VCIsEastAsian(unichar character) {
    return (character >= 0x3040 && character <= 0x30ff) ||
           (character >= 0x3400 && character <= 0x9fff) ||
           (character >= 0xac00 && character <= 0xd7af);
}

static BOOL VCHasText(NSString *value) {
    NSString *normalized = VCNormalized(value);
    if (normalized.length == 0) return NO;
    if (normalized.length >= 2) return YES;
    return VCIsEastAsian([normalized characterAtIndex:0]);
}

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
    NSMutableArray<NSDictionary *> *lines = [NSMutableArray array];
    for (VNRecognizedTextObservation *observation in observations) {
        CGRect box = observation.boundingBox;
        if (CGRectGetMidY(box) > 0.22 || CGRectGetHeight(box) > 0.14) continue;
        NSMutableArray<NSDictionary *> *candidates = [NSMutableArray array];
        for (VNRecognizedText *candidate in [observation topCandidates:3]) {
            if (candidate.confidence < 0.35 || !VCHasText(candidate.string)) continue;
            [candidates addObject:@{
                @"text" : candidate.string,
                @"confidence" : @(candidate.confidence)
            }];
        }
        if (candidates.count == 0) continue;
        [lines addObject:@{
            @"x" : @(CGRectGetMinX(box)),
            @"y" : @(CGRectGetMinY(box)),
            @"w" : @(CGRectGetWidth(box)),
            @"h" : @(CGRectGetHeight(box)),
            @"candidates" : candidates
        }];
    }
    return @{ @"text" : @"", @"confidence" : @0, @"lines" : lines };
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
            for (NSDictionary *line in recognized[@"lines"]) {
                NSDictionary *best = [line[@"candidates"] firstObject];
                NSString *key = VCNormalized(best[@"text"]);
                if (key.length >= 1 && [best[@"confidence"] floatValue] >= 0.42) {
                    hits++;
                    [unique addObject:key];
                    break;
                }
            }
        }
    }
    return hits >= 4 && unique.count >= 3;
}

int main(int argc, const char *argv[]) {
    @autoreleasepool {
        if (argc < 3) {
            fprintf(stderr, "usage: subtitle-ocr <video> <output.srt> [interval]\n");
            fprintf(stderr, "writes <output>.observations.json with one raw sample per frame\n");
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
        request.regionOfInterest = CGRectMake(0.06, 0.0, 0.88, 0.28);
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
        NSMutableArray<NSDictionary *> *samples = [NSMutableArray array];
        for (NSInteger index = 0; index < total; index++) {
            @autoreleasepool {
                double second = MIN(duration - 0.01, index * interval);
                CGImageRef frame = VCCopyFrame(generator, second);
                NSDictionary *recognized = frame ? VCRecognize(frame, request) : @{ @"text" : @"", @"confidence" : @0 };
                if (frame) CGImageRelease(frame);
                NSMutableDictionary *sample = [recognized mutableCopy];
                sample[@"t"] = @(second);
                [samples addObject:sample];
                if (index % 20 == 0) {
                    VCProgress(5 + 92.0 * index / MAX(total, 1),
                               [NSString stringWithFormat:@"正在本地识别画面字幕 %ld/%ld", (long)index, (long)total]);
                }
            }
        }
        NSInteger textFrames = 0;
        NSInteger minimum = duration >= 1200 ? 30 : MAX(5, (NSInteger)(duration / 240));
        NSMutableIndexSet *quarters = [NSMutableIndexSet indexSet];
        for (NSDictionary *sample in samples) {
            NSArray *lines = sample[@"lines"];
            if (![lines isKindOfClass:NSArray.class] || lines.count == 0) continue;
            textFrames++;
            double start = [sample[@"t"] doubleValue];
            [quarters addIndex:MIN(3, (NSUInteger)((start / duration) * 4))];
        }
        if (textFrames < minimum || (duration >= 1200 && quarters.count < 3)) {
            VCProgress(100, @"画面文字不足以构成连续字幕，已放弃 OCR 结果");
            return 3;
        }
        NSString *observationsPath = [[outputPath stringByDeletingPathExtension]
            stringByAppendingString:@".observations.json"];
        NSData *data = [NSJSONSerialization dataWithJSONObject:samples options:0 error:nil];
        [[NSFileManager defaultManager] createDirectoryAtPath:observationsPath.stringByDeletingLastPathComponent
                                  withIntermediateDirectories:YES attributes:nil error:nil];
        if (!data || ![data writeToFile:observationsPath atomically:YES]) return 2;
        VCProgress(100, [NSString stringWithFormat:@"画面采样完成，共 %lu 帧，等待多帧投票", (unsigned long)samples.count]);
        return 0;
    }
}
