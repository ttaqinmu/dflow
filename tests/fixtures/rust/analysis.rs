/// Changed documentation must change the symbol fingerprint.
#[inline]
pub async fn fetch<T: Copy>(value: T) -> T {
    let copied = value;
    copied
}

mod inline {
    pub fn inner() -> usize { 4 }
}

struct Service;
impl Service {
    fn build() -> Self { Service }
    fn run(&self) { inline::inner(); }
}

trait Dispatch {
    fn send(&self);
}

fn caller() {
    let _service = Service::build();
    fetch(2);
    log!("opaque");
}
