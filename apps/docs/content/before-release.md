# Before the first release

Cavea has not been released. Until it is, the server image and the seat-map package that the [quick start](quick-start.md) names are not published, and you build both from the repository. This page says how, and what to change in the quick start. It goes away at the first release.

What is not published yet:

| The quick start names | What it is | Until it is published |
|---|---|---|
| `{{image}}` | The server's container image | Built from the repository with Docker |
| `{{renderer_script}}` | The seat map's script-tag build, from the npm package `{{renderer_package}}` | Built from the repository, then served beside your page |

## Build the server image

You need Git and Docker. The build compiles the server and its console from source; the first build takes 10 to 20 minutes, later ones a few.

```sh
git clone {{repository}}.git
cd cavea
docker buildx build -f deploy/Dockerfile -t cavea-server:local --load .
```

In the quick start's `compose.yaml`, use that image:

```yaml check=run:quickstart
  server:
    image: cavea-server:local
```

## Build the seat map

Building the seat map needs the repository's toolchain: Rust with the WebAssembly target, bun and `wasm-opt`. The repository's Nix flake provides all of them; without Nix, see the repository's `README.md` for the same tools by hand. From the `cavea` directory:

```sh
nix develop -c bun install --frozen-lockfile
nix develop -c cargo xtask build-web
nix develop -c bun run --cwd packages/renderer build
```

That writes the package to `packages/renderer/dist/`. The script-tag build needs three of its files side by side: copy them into the quick start's `public/cavea/` directory (the commands below assume the `cavea` clone and the `cavea-quickstart` directory are side by side):

```sh
mkdir -p ../cavea-quickstart/public/cavea
cp packages/renderer/dist/cavea.min.js packages/renderer/dist/cavea_web_bg.wasm \
  packages/renderer/dist/glyphs.png ../cavea-quickstart/public/cavea/
```

In the quick start's `public/index.html`, load the script from there:

```html check=run:quickstart
<script src="cavea/cavea.min.js"></script>
```

The script fetches the WebAssembly and the glyph atlas from beside itself, so the page's web server now serves all three from the page's own origin.

## The npm packages

The [examples](examples/index.md) and the guides install `{{renderer_package}}`, `@cavea/react` and `{{server_sdk_package}}` from npm. Until they are published, build them and pack each as a tarball, from the `cavea` directory:

```sh
nix develop -c bun install --frozen-lockfile
nix develop -c cargo xtask build-web
mkdir -p ../cavea-packages
for p in renderer react server-sdk; do
  (cd packages/$p && nix develop -c bun run build && cd dist && nix develop -c bun pm pack --destination ../../../../cavea-packages)
done
```

That writes `cavea-renderer-0.0.0.tgz`, `cavea-react-0.0.0.tgz` and `cavea-server-sdk-0.0.0.tgz` into `cavea-packages`, beside the clone. In a copy of an example, install the ones its `package.json` names from there instead of from npm (the command below has the Next.js example's three, with the copy beside the clone), then carry on with its README:

```sh
npm install ../cavea-packages/cavea-renderer-0.0.0.tgz ../cavea-packages/cavea-react-0.0.0.tgz ../cavea-packages/cavea-server-sdk-0.0.0.tgz
```

This replaces the versions in its `package.json` with the tarballs' paths; do not commit that change.
