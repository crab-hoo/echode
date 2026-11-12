import Accelerate
import Foundation
import Metal

let frames = 48_000 * 60
let bucket = 64
let columns = frames / bucket
var samples = [Float](repeating: 0, count: frames)
for index in 0..<frames {
    samples[index] = sinf(Float(index) * 0.01)
}

func time(_ name: String, _ body: () -> Void) {
    _ = body()
    let started = Date()
    body()
    let elapsed = Date().timeIntervalSince(started)
    print(String(format: "%@: %.4f s", name, elapsed))
}

time("cpu loop") {
    var lo = Float.infinity
    var hi = -Float.infinity
    for column in 0..<columns {
        var minValue = samples[column * bucket]
        var maxValue = minValue
        for offset in 1..<bucket {
            let sample = samples[column * bucket + offset]
            minValue = min(minValue, sample)
            maxValue = max(maxValue, sample)
        }
        lo = min(lo, minValue)
        hi = max(hi, maxValue)
    }
    precondition(lo < hi)
}

time("vdsp minv/maxv per column") {
    var lo = Float.infinity
    var hi = -Float.infinity
    samples.withUnsafeBufferPointer { input in
        for column in 0..<columns {
            var minValue: Float = 0
            var maxValue: Float = 0
            vDSP_minv(input.baseAddress! + column * bucket, 1, &minValue, vDSP_Length(bucket))
            vDSP_maxv(input.baseAddress! + column * bucket, 1, &maxValue, vDSP_Length(bucket))
            lo = min(lo, minValue)
            hi = max(hi, maxValue)
        }
    }
    precondition(lo < hi)
}

let source = """
    #include <metal_stdlib>
    using namespace metal;
    kernel void peaks(device const float *samples [[buffer(0)]],
                      device float2 *out [[buffer(1)]],
                      constant uint &bucket [[buffer(2)]],
                      uint id [[thread_position_in_grid]]) {
        uint start = id * bucket;
        float lo = samples[start];
        float hi = lo;
        for (uint offset = 1; offset < bucket; ++offset) {
            float sample = samples[start + offset];
            lo = min(lo, sample);
            hi = max(hi, sample);
        }
        out[id] = float2(lo, hi);
    }
    """
guard let device = MTLCreateSystemDefaultDevice(), let queue = device.makeCommandQueue() else {
    fputs("no metal device\n", stderr)
    exit(1)
}
let library = try device.makeLibrary(source: source, options: nil)
let function = library.makeFunction(name: "peaks")!
let pipeline = try device.makeComputePipelineState(function: function)
let sampleBuffer = samples.withUnsafeBytes {
    device.makeBuffer(bytes: $0.baseAddress!, length: $0.count, options: .storageModeShared)!
}
let outBuffer = device.makeBuffer(length: columns * MemoryLayout<SIMD2<Float>>.stride, options: .storageModeShared)!
var bucketValue = UInt32(bucket)
time("metal compute") {
    guard let command = queue.makeCommandBuffer(), let encoder = command.makeComputeCommandEncoder() else { return }
    encoder.setComputePipelineState(pipeline)
    encoder.setBuffer(sampleBuffer, offset: 0, index: 0)
    encoder.setBuffer(outBuffer, offset: 0, index: 1)
    encoder.setBytes(&bucketValue, length: MemoryLayout<UInt32>.stride, index: 2)
    let width = pipeline.threadExecutionWidth
    let threads = MTLSize(width: columns, height: 1, depth: 1)
    let group = MTLSize(width: width, height: 1, depth: 1)
    encoder.dispatchThreads(threads, threadsPerThreadgroup: group)
    encoder.endEncoding()
    command.commit()
    command.waitUntilCompleted()
}
print(device.name)
