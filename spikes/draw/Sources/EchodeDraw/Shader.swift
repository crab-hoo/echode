let noteShader = """
    #include <metal_stdlib>
    using namespace metal;

    struct Instance {
        float4 rect;
        uint rgba;
        uint pad0;
        uint pad1;
        uint pad2;
    };

    struct VertexOut {
        float4 position [[position]];
        float4 color;
    };

    vertex VertexOut note_vertex(
        uint vertexID [[vertex_id]],
        uint instanceID [[instance_id]],
        constant Instance *instances [[buffer(0)]],
        constant float2 &viewport [[buffer(1)]]
    ) {
        const float2 corners[6] = {
            float2(0, 0), float2(1, 0), float2(0, 1),
            float2(1, 0), float2(1, 1), float2(0, 1),
        };
        Instance instance = instances[instanceID];
        float2 pixel = instance.rect.xy + corners[vertexID] * instance.rect.zw;
        float2 ndc = float2(
            pixel.x / viewport.x * 2.0 - 1.0,
            1.0 - pixel.y / viewport.y * 2.0
        );
        VertexOut out;
        out.position = float4(ndc, 0, 1);
        float r = float((instance.rgba >> 16) & 0xff) / 255.0;
        float g = float((instance.rgba >> 8) & 0xff) / 255.0;
        float b = float(instance.rgba & 0xff) / 255.0;
        float a = float((instance.rgba >> 24) & 0xff) / 255.0;
        out.color = float4(r, g, b, a);
        return out;
    }

    fragment float4 note_fragment(VertexOut in [[stage_in]]) {
        return in.color;
    }
    """
