use ruby_lsp_rs::uri::{from_path, require_path, to_path};

fn uri_path(uri: &str) -> &str {
    uri.strip_prefix("file://").unwrap()
}

#[test]
fn test_from_path_on_unix() {
    let uri = from_path("/some/unix/path/to/file.rb");
    assert_eq!("/some/unix/path/to/file.rb", uri_path(&uri));
}

#[test]
fn test_from_path_on_windows() {
    let uri = from_path("C:/some/windows/path/to/file.rb");
    assert_eq!("/C%3A/some/windows/path/to/file.rb", uri_path(&uri));
}

#[test]
fn test_from_path_on_windows_with_lowercase_drive() {
    let uri = from_path("c:/some/windows/path/to/file.rb");
    assert_eq!("/c%3A/some/windows/path/to/file.rb", uri_path(&uri));
}

#[test]
fn test_to_standardized_path_on_unix() {
    let uri = from_path("/some/unix/path/to/file.rb");
    assert_eq!(uri_path(&uri), to_path(&uri).unwrap());
}

#[test]
fn test_to_standardized_path_on_windows() {
    let uri = from_path("C:/some/windows/path/to/file.rb");
    assert_eq!("C:/some/windows/path/to/file.rb", to_path(&uri).unwrap());
}

#[test]
fn test_to_standardized_path_on_windows_with_lowercase_drive() {
    let uri = from_path("c:/some/windows/path/to/file.rb");
    assert_eq!("c:/some/windows/path/to/file.rb", to_path(&uri).unwrap());
}

#[test]
fn test_to_standardized_path_on_windows_with_received_uri() {
    assert_eq!("c:/some/windows/path/to/file.rb", to_path("file:///c%3A/some/windows/path/to/file.rb").unwrap());
}

#[test]
fn test_plus_signs_are_properly_unescaped() {
    let path = "/opt/rubies/3.3.0/lib/ruby/3.3.0+0/pathname.rb";
    assert_eq!(path, to_path(&from_path(path)).unwrap());
}

#[test]
fn test_from_path_windows_long_file_paths() {
    let uri = from_path("//?/C:/hostedtoolcache/windows/Ruby/3.3.1/x64/lib/ruby/3.3.0/open-uri.rb");
    assert_eq!("C:/hostedtoolcache/windows/Ruby/3.3.1/x64/lib/ruby/3.3.0/open-uri.rb", to_path(&uri).unwrap());
}

#[test]
fn test_from_path_computes_require_path_when_load_path_entry_is_given() {
    assert_eq!("to/file", require_path("/some/unix/path/to/file.rb", "/some/unix/path"));
}

#[test]
fn test_from_path_escapes_colon_characters() {
    let uri = from_path("c:/some/windows/path with/spaces/file.rb");
    assert_eq!("c:/some/windows/path with/spaces/file.rb", to_path(&uri).unwrap());
    assert_eq!("file:///c%3A/some/windows/path%20with/spaces/file.rb", uri);
}

#[test]
fn test_from_path_with_unicode_characters() {
    let path = "/path/with/unicode/文件.rb";
    let uri = from_path(path);
    assert_eq!(path, to_path(&uri).unwrap());
    assert_eq!("file:///path/with/unicode/%E6%96%87%E4%BB%B6.rb", uri);
}
