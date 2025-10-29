use std::any::Any;

trait MyTrait: Any {
    fn do_work(&mut self);
}

struct MyStruct {
    calls: usize,
}

impl MyTrait for MyStruct {
    #[inline(never)]
    fn do_work(&mut self) {
        self.calls += 1;
    }
}

struct MyStruct2 {
    calls: usize,
}

impl MyTrait for MyStruct2 {
    #[inline(never)]
    fn do_work(&mut self) {
        self.calls += 2;
    }
}

fn bench_call_overhead(s: &mut Box<MyStruct>) {
    let now = std::time::Instant::now();
    for _ in 0..1000000 {
        s.do_work();
    }
    let end = std::time::Instant::now();
    println!("1M direct calls in {:?}", end - now);
}

fn bench_dyn_call_overhead(s: &mut Box<dyn MyTrait>) {
    let now = std::time::Instant::now();
    for _ in 0..1000000 {
        s.do_work();
    }
    let end = std::time::Instant::now();
    println!("1M dyn calls in in {:?}", end - now);
}

fn main() {
    let mut s = Box::new(MyStruct { calls: 0 });
    bench_call_overhead(&mut s);
    println!("? {}", s.calls);

    let s = Box::new(MyStruct { calls: 0 });
    let mut s_dyn: Box<dyn MyTrait> = s;
    bench_dyn_call_overhead(&mut s_dyn);
    let s_any: Box<dyn Any> = s_dyn;
    let s: Box<MyStruct> = s_any.downcast().unwrap();
    println!("? {}", s.calls);

    let s = Box::new(MyStruct2 { calls: 0 });
    let mut s_dyn: Box<dyn MyTrait> = s;
    bench_dyn_call_overhead(&mut s_dyn);
    let s_any: Box<dyn Any> = s_dyn;
    let s: Box<MyStruct2> = s_any.downcast().unwrap();
    println!("? {}", s.calls);
}
