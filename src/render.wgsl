struct Uniforms {
    view_proj: mat4x4<f32>,
    eye: vec4<f32>,
    grid: vec4<u32>,
}

@group(0) @binding(0) var<uniform> uniforms: Uniforms;
@group(1) @binding(0) var<storage, read> cloth: array<vec4<f32>>;

struct MeshIn {
    @location(0) position: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) color: vec4<f32>,
}

struct MeshOut {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) world_position: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) color: vec4<f32>,
}

struct ClothOut {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) world_position: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) uv: vec2<f32>,
}

// Lights a surface with diffuse, ambient and a soft highlight, flipping the normal to face the camera.
fn shade(base: vec3<f32>, world_position: vec3<f32>, normal: vec3<f32>, gloss: f32) -> vec3<f32> {
    let light_dir = normalize(vec3<f32>(0.4, 1.0, 0.3));
    let to_eye = normalize(uniforms.eye.xyz - world_position);

    var n = normalize(normal);
    if (dot(n, to_eye) < 0.0) {
        n = -n;
    }

    let diffuse = max(dot(n, light_dir), 0.0);
    let half_vec = normalize(light_dir + to_eye);
    let spec = pow(max(dot(n, half_vec), 0.0), 48.0) * gloss;

    return base * (diffuse * 0.85 + 0.25) + vec3<f32>(spec);
}

// Reads a cloth particle index from a grid coordinate, clamped to the edges.
fn cloth_index(x: i32, y: i32) -> u32 {
    let n = i32(uniforms.grid.x);
    return u32(clamp(y, 0, n - 1) * n + clamp(x, 0, n - 1));
}

// Estimates the cloth surface normal from the four neighbouring particles.
fn cloth_normal(id: u32) -> vec3<f32> {
    let n = uniforms.grid.x;
    let x = i32(id % n);
    let y = i32(id / n);

    let left = cloth[cloth_index(x - 1, y)].xyz;
    let right = cloth[cloth_index(x + 1, y)].xyz;
    let up = cloth[cloth_index(x, y - 1)].xyz;
    let down = cloth[cloth_index(x, y + 1)].xyz;

    return cross(right - left, down - up);
}

// Transforms a sphere or ground vertex to clip space.
@vertex
fn vs_mesh(in: MeshIn) -> MeshOut {
    var out: MeshOut;
    out.clip_position = uniforms.view_proj * vec4<f32>(in.position, 1.0);
    out.world_position = in.position;
    out.normal = in.normal;
    out.color = in.color;
    return out;
}

// Colors the sphere and ground with their vertex color and lighting.
@fragment
fn fs_mesh(in: MeshOut) -> @location(0) vec4<f32> {
    return vec4<f32>(shade(in.color.rgb, in.world_position, in.normal, in.color.a), 1.0);
}

// Places a cloth vertex using the particle position stored in the storage buffer.
@vertex
fn vs_cloth(@builtin(vertex_index) id: u32) -> ClothOut {
    let n = uniforms.grid.x;
    let position = cloth[id].xyz;

    var out: ClothOut;
    out.clip_position = uniforms.view_proj * vec4<f32>(position, 1.0);
    out.world_position = position;
    out.normal = cloth_normal(id);
    out.uv = vec2<f32>(f32(id % n), f32(id / n)) / f32(n - 1u);
    return out;
}

// Colors the solid cloth with a checker pattern so stretching is easy to see.
@fragment
fn fs_cloth(in: ClothOut) -> @location(0) vec4<f32> {
    let cell = floor(in.uv * 16.0);
    let odd = (cell.x + cell.y) % 2.0;
    let base = mix(vec3<f32>(0.08, 0.40, 0.70), vec3<f32>(0.85, 0.88, 0.95), odd);
    return vec4<f32>(shade(base, in.world_position, in.normal, 0.1), 1.0);
}

// Places a net line endpoint using the particle position.
@vertex
fn vs_net(@builtin(vertex_index) id: u32) -> @builtin(position) vec4<f32> {
    return uniforms.view_proj * vec4<f32>(cloth[id].xyz, 1.0);
}

// Colors every net line dark.
@fragment
fn fs_net() -> @location(0) vec4<f32> {
    return vec4<f32>(0.01, 0.01, 0.02, 1.0);
}
