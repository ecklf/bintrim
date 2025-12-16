<p align="center">
  <a align="center" href="https://vercel.com">
    <img src="public/logo.png" height="128" alt="bintrim logo" />
    <h3 align="center">bintrim</h3>
  </a>
</p>

<div align="center">
A CLI utility for stripping legacy <code>x86_64</code> architecture from macOS universal binaries
</div>

<br/>
<br/>

![bintrim app](public/app.png)

## Installation

```bash
brew tap ecklf/bintrim
brew install bintrim
```

Or install directly:

```bash
brew install ecklf/bintrim/bintrim
```

## License

MIT

<details><summary>Release</summary>
```sh
git tag v0.1.0
git push origin v0.1.0
curl -sL https://github.com/ecklf/bintrim/archive/refs/tags/v0.1.0.tar.gz | shasum -a 256
```
</details>
