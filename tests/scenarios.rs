mod support;

use support::Repository;

#[test]
fn kao_says_hello_to_a_duck() {
    let repo = Repository::new("duck pond");
    repo.write("duck.txt", "quack\n");
    repo.commit("Add duck");

    assert_eq!(repo.kao(&[]), "Hello, world!\n");
    assert_eq!(repo.read("duck.txt"), "quack\n");
    assert_eq!(repo.status(), "");
}
