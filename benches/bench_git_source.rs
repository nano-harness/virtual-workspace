use criterion::{criterion_group, criterion_main, BatchSize, BenchmarkId, Criterion};
use std::path::Path;
use std::process::Command;
use vwsd::git_ops;
use vwsd::git_source;
use vwsd::model::{RemoteGit, Source};
use vwsd::util::expand_tilde;

fn bench_expand_tilde(c: &mut Criterion) {
    let mut group = c.benchmark_group("expand_tilde");
    group.bench_function("home_only", |b| b.iter(|| expand_tilde("~")));
    group.bench_function("with_subpath", |b| {
        b.iter(|| expand_tilde("~/projects/my-workspace/src"))
    });
    group.finish();
}

fn bench_parse_git_url(c: &mut Criterion) {
    let mut group = c.benchmark_group("parse_git_url");
    for (name, url) in [
        ("https", "https://github.com/my-org/my-repo.git"),
        ("ssh_shorthand", "git@github.com:my-org/my-repo.git"),
        ("ssh_full", "ssh://git@github.com/my-org/my-repo.git"),
    ] {
        group.bench_with_input(BenchmarkId::new("parse", name), &url, |b, input| {
            b.iter(|| vwsd::url_parser::parse_git_url(input))
        });
    }
    group.finish();
}

fn make_origin(td: &Path) -> String {
    let origin = td.join("origin.git");
    Command::new("git")
        .args(["init", "--bare", "-q"])
        .arg(&origin)
        .output()
        .unwrap();
    let seed = td.join("seed");
    git_ops::clone(origin.to_str().unwrap(), &seed, None).unwrap();
    for (key, value) in [("user.email", "t@t.com"), ("user.name", "T")] {
        Command::new("git")
            .args(["config", key, value])
            .current_dir(&seed)
            .output()
            .unwrap();
    }
    for i in 0..5 {
        std::fs::write(seed.join(format!("file{i}.txt")), format!("content {i}")).unwrap();
        git_ops::add_all(&seed).unwrap();
        git_ops::commit(&seed, &format!("commit {i}")).unwrap();
    }
    git_ops::push(&seed).unwrap();
    origin.to_string_lossy().to_string()
}

fn make_source(url: &str) -> Source {
    Source {
        path: String::new(),
        remote_git: Some(RemoteGit {
            url: url.to_string(),
            branch: None,
        }),
        mount_as: Some("repo".to_string()),
    }
}

fn bench_prepare_remote_source_warm(c: &mut Criterion) {
    let td = tempfile::tempdir().unwrap();
    let url = make_origin(td.path());
    let root = td.path().join("cache");
    let target = td.path().join("workspace/repo");
    let mut src = make_source(&url);
    git_source::prepare_remote_source(&mut src, "bench-ws", &target, Some(&root)).unwrap();

    c.bench_function("git_source/prepare_remote_source_warm", |b| {
        b.iter(|| {
            let mut src = make_source(&url);
            git_source::prepare_remote_source(&mut src, "bench-ws", &target, Some(&root)).unwrap()
        })
    });
}

fn bench_prepare_remote_source_cold(c: &mut Criterion) {
    let td = tempfile::tempdir().unwrap();
    let url = make_origin(td.path());
    c.bench_function("git_source/prepare_remote_source_cold", |b| {
        b.iter_batched(
            || tempfile::tempdir().unwrap(),
            |cache| {
                let mut src = make_source(&url);
                let target = cache.path().join("workspace/repo");
                git_source::prepare_remote_source(&mut src, "bench-ws", &target, Some(cache.path()))
                    .unwrap()
            },
            BatchSize::SmallInput,
        )
    });
}

criterion_group!(
    benches,
    bench_expand_tilde,
    bench_parse_git_url,
    bench_prepare_remote_source_warm,
    bench_prepare_remote_source_cold
);
criterion_main!(benches);
