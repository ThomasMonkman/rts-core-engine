-- Moho vector helpers. Implemented in Luau on purpose: for leaf math like this
-- a Luau call is ~3x faster than a native callback (measured: VDist3 32ns vs
-- 95ns per call), and only IEEE basic ops + sqrt are used, so results are
-- bit-identical across platforms.
local vector_mt = ...
local sqrt, setmetatable = math.sqrt, setmetatable

function VAdd(a, b)
    return setmetatable({ a[1] + b[1], a[2] + b[2], a[3] + b[3] }, vector_mt)
end

function VDiff(a, b)
    return setmetatable({ a[1] - b[1], a[2] - b[2], a[3] - b[3] }, vector_mt)
end

function VMult(v, s)
    return setmetatable({ v[1] * s, v[2] * s, v[3] * s }, vector_mt)
end

function VDist2(x1, y1, x2, y2)
    local dx, dy = x1 - x2, y1 - y2
    return sqrt(dx * dx + dy * dy)
end

function VDist2Sq(x1, y1, x2, y2)
    local dx, dy = x1 - x2, y1 - y2
    return dx * dx + dy * dy
end

-- Moho's VDist3 reads a nil vector as {0, 0, 0} (real-game probe:
-- `VDist3(nil, {1, 2, 3})` is 3.74); VDist3Sq and VDist2 raise on nil.
local zero = table.freeze({ 0, 0, 0 })

function VDist3(a, b)
    if a == nil then a = zero end
    if b == nil then b = zero end
    local dx, dy, dz = a[1] - b[1], a[2] - b[2], a[3] - b[3]
    return sqrt(dx * dx + dy * dy + dz * dz)
end

function VDist3Sq(a, b)
    local dx, dy, dz = a[1] - b[1], a[2] - b[2], a[3] - b[3]
    return dx * dx + dy * dy + dz * dz
end

function VDot(a, b)
    return a[1] * b[1] + a[2] * b[2] + a[3] * b[3]
end

function VPerpDot(a, b)
    return a[3] * b[1] - a[1] * b[3]
end

-- Operators of moho's `Vector` / `Quaternion` (`/engine/Core.lua`). A table
-- with a 4th component is a quaternion {x, y, z, w}.
-- TODO: `Vector * Vector` and `Vector * Quaternion` are annotated but their
-- semantics are unknown; they raise until something needs them.
local function scale(v, s)
    if v[4] ~= nil then
        return setmetatable({ v[1] * s, v[2] * s, v[3] * s, v[4] * s }, vector_mt)
    end
    return setmetatable({ v[1] * s, v[2] * s, v[3] * s }, vector_mt)
end

vector_mt.__mul = function(a, b)
    if type(b) == "number" then return scale(a, b) end
    if type(a) == "number" then return scale(b, a) end
    if a[4] ~= nil and b[4] ~= nil then
        -- Hamilton product a * b
        local ax, ay, az, aw = a[1], a[2], a[3], a[4]
        local bx, by, bz, bw = b[1], b[2], b[3], b[4]
        return setmetatable({
            aw * bx + ax * bw + ay * bz - az * by,
            aw * by - ax * bz + ay * bw + az * bx,
            aw * bz + ax * by - ay * bx + az * bw,
            aw * bw - ax * bx - ay * by - az * bz,
        }, vector_mt)
    end
    error("unsupported Vector/Quaternion multiplication", 2)
end

vector_mt.__add = function(a, b)
    return setmetatable({ a[1] + b[1], a[2] + b[2], a[3] + b[3] }, vector_mt)
end

vector_mt.__unm = function(a)
    return setmetatable({ -a[1], -a[2], -a[3] }, vector_mt)
end
