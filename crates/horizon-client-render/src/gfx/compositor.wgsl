// The splitscreen compositor's draw path: one view's frame on a window surface that takes
// no texture copies, texel for pixel. The viewport is the whole frame's place in the window.

@group(0) @binding(0) var frame: texture_2d<f32>;

struct Corner {
    @builtin(position) position: vec4<f32>,
    // In frames from the frame's top-left corner.
    @location(0) at: vec2<f32>,
}

// One triangle that covers the viewport.
@vertex
fn vertex(@builtin(vertex_index) index: u32) -> Corner {
    let at = vec2<f32>(f32((index << 1u) & 2u), f32(index & 2u));
    return Corner(vec4<f32>(at.x * 2.0 - 1.0, 1.0 - at.y * 2.0, 0.0, 1.0), at);
}

@fragment
fn fragment(corner: Corner) -> @location(0) vec4<f32> {
    let texel = vec2<i32>(corner.at * vec2<f32>(textureDimensions(frame)));
    return textureLoad(frame, texel, 0);
}
