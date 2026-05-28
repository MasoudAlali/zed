#!/bin/zsh

echo "stable" > crates/zed/RELEASE_CHANNEL

ZED_BUNDLE=true ZED_RELEASE_CHANNEL=stable cargo build --release --package zed --package cli

cd crates/zed
cp Cargo.toml Cargo.toml.backup
sed -i '' "s/package.metadata.bundle-stable/package.metadata.bundle/" Cargo.toml
ZED_BUNDLE=true ZED_RELEASE_CHANNEL=stable cargo bundle --release --select-workspace-root
mv Cargo.toml.backup Cargo.toml
cd ../..

app_path="./target/release/bundle/osx/Zed.app"

cp ./target/release/zed "${app_path}/Contents/MacOS/zed"
cp ./target/release/cli "${app_path}/Contents/MacOS/cli"

if [[ -f "crates/zed/resources/Document.icns" ]]; then
    mkdir -p "${app_path}/Contents/Resources"
    cp crates/zed/resources/Document.icns "${app_path}/Contents/Resources/Document.icns"
fi

codesign --force --deep --entitlements crates/zed/resources/zed.entitlements --sign - "${app_path}"

rm -rf /Applications/Zed.app
mv "${app_path}" /Applications/
