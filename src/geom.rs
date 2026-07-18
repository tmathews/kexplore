#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Point {
    pub x: f32,
    pub y: f32,
}

impl Point {
    pub const ZERO: Point = Point { x: 0.0, y: 0.0 };

    pub fn new(x: f32, y: f32) -> Point {
        Point { x, y }
    }

    pub fn add(self, o: Point) -> Point {
        Point::new(self.x + o.x, self.y + o.y)
    }

    pub fn sub(self, o: Point) -> Point {
        Point::new(self.x - o.x, self.y - o.y)
    }

    pub fn scale(self, s: f32) -> Point {
        Point::new(self.x * s, self.y * s)
    }

    pub fn lerp(self, target: Point, t: f32) -> Point {
        Point::new(self.x + (target.x - self.x) * t, self.y + (target.y - self.y) * t)
    }

    pub fn length(self) -> f32 {
        (self.x * self.x + self.y * self.y).sqrt()
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Rect {
    pub min: Point,
    pub max: Point,
}

impl Rect {
    pub const ZERO: Rect = Rect { min: Point::ZERO, max: Point::ZERO };

    pub fn from_xywh(x: f32, y: f32, w: f32, h: f32) -> Rect {
        Rect { min: Point::new(x, y), max: Point::new(x + w, y + h) }
    }

    pub fn width(self) -> f32 {
        self.max.x - self.min.x
    }

    pub fn height(self) -> f32 {
        self.max.y - self.min.y
    }

    pub fn center(self) -> Point {
        Point::new((self.min.x + self.max.x) * 0.5, (self.min.y + self.max.y) * 0.5)
    }

    pub fn offset(self, p: Point) -> Rect {
        Rect { min: self.min.add(p), max: self.max.add(p) }
    }

    pub fn contains(self, p: Point) -> bool {
        p.x >= self.min.x && p.x <= self.max.x && p.y >= self.min.y && p.y <= self.max.y
    }

    pub fn intersects(self, o: Rect) -> bool {
        self.min.x <= o.max.x && o.min.x <= self.max.x && self.min.y <= o.max.y && o.min.y <= self.max.y
    }

    pub fn intersect(self, o: Rect) -> Option<Rect> {
        let r = Rect {
            min: Point::new(self.min.x.max(o.min.x), self.min.y.max(o.min.y)),
            max: Point::new(self.max.x.min(o.max.x), self.max.y.min(o.max.y)),
        };
        (r.min.x < r.max.x && r.min.y < r.max.y).then_some(r)
    }
}
