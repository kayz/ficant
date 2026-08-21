# `@ficant/contracts-generated`

This private `0.0.0` package contains the tracked TypeScript output generated from FICANT's authoritative protobuf sources under `interface/`. Do not edit files under `src/` by hand and do not publish this package to a registry.

Create the reproducible local tarball from the repository root with:

```powershell
pwsh -NoProfile -NonInteractive -File .\scripts\package-contracts.ps1
```

The command writes the ignored artifact to `web-dm/packages/contracts-generated/dist/` and reports the descriptor, generated-source tree, and package SHA-256 digests. A neighboring project may install the exact artifact by its local path, for example:

```powershell
corepack pnpm@10.12.4 add --offline 'C:\git\ficant\web-dm\packages\contracts-generated\dist\ficant-contracts-generated-0.0.0.tgz'
```

Generated modules are consumed through package subpaths, such as `@ficant/contracts-generated/ficant/portfolio/v1/portfolio_pb`. FICANT runtime services never depend on this package.
