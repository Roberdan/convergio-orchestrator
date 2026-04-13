# Changelog

## [0.1.8](https://github.com/Roberdan/convergio-orchestrator/compare/v0.1.7...v0.1.8) (2026-04-13)


### Bug Fixes

* plan executor respects assignee:manual — stops auto-pause/fail ([#984](https://github.com/Roberdan/convergio-orchestrator/issues/984)) ([2bde4d4](https://github.com/Roberdan/convergio-orchestrator/commit/2bde4d4239a71d7595787f2402158c8cea18457b))
* plan executor respects assignee:manual ([#984](https://github.com/Roberdan/convergio-orchestrator/issues/984)) ([deb4294](https://github.com/Roberdan/convergio-orchestrator/commit/deb4294b793ae68dfc9a498db7ad672d1e3d67ed))
* wave_pr_guard skips cross-repo waves with direct_to_main ([#986](https://github.com/Roberdan/convergio-orchestrator/issues/986)) ([78268a8](https://github.com/Roberdan/convergio-orchestrator/commit/78268a8919ef2f2a4fe462fae229ba36ec35ee2b))

## [0.1.7](https://github.com/Roberdan/convergio-orchestrator/compare/v0.1.6...v0.1.7) (2026-04-13)


### Bug Fixes

* allow plan recovery from failed state and prevent premature plan failure ([d3b75fb](https://github.com/Roberdan/convergio-orchestrator/commit/d3b75fb19c7aa2e5e25f59cf7fb32c67e202bbcd))
* plan recovery from failed state ([#970](https://github.com/Roberdan/convergio-orchestrator/issues/970)) ([cd813c4](https://github.com/Roberdan/convergio-orchestrator/commit/cd813c42a2d6c5d8097e95c60ad679c417b306f1))

## [0.1.6](https://github.com/Roberdan/convergio-orchestrator/compare/v0.1.5...v0.1.6) (2026-04-13)


### Bug Fixes

* remove redundant path validation on checkpoint save ([fbd6839](https://github.com/Roberdan/convergio-orchestrator/commit/fbd68393bb34d49a2860bb0d34a353479955fbc4))
* remove redundant path validation on checkpoint save ([#10](https://github.com/Roberdan/convergio-orchestrator/issues/10)) ([2f11c07](https://github.com/Roberdan/convergio-orchestrator/commit/2f11c075e893c4f64367ece5f98d8707caa5fc69))

## [0.1.5](https://github.com/Roberdan/convergio-orchestrator/compare/v0.1.4...v0.1.5) (2026-04-13)


### Bug Fixes

* fix malformed convergio-ipc dependency in Cargo.toml ([#8](https://github.com/Roberdan/convergio-orchestrator/issues/8)) ([ecbe854](https://github.com/Roberdan/convergio-orchestrator/commit/ecbe854398bb429e4195e8aa4edccd7d218a2ac9))

## [0.1.4](https://github.com/Roberdan/convergio-orchestrator/compare/v0.1.3...v0.1.4) (2026-04-13)


### Bug Fixes

* **deps:** update convergio-ipc to v0.1.6 (SDK v0.1.9 aligned) ([13a6732](https://github.com/Roberdan/convergio-orchestrator/commit/13a673292e94ede813678049b93d10394f3e3482))

## [0.1.3](https://github.com/Roberdan/convergio-orchestrator/compare/v0.1.2...v0.1.3) (2026-04-13)


### Features

* adapt convergio-orchestrator + evidence bundle for standalone repo ([20a35b9](https://github.com/Roberdan/convergio-orchestrator/commit/20a35b9da7347f35faf31eb4c2694c4f8979b7dc))


### Bug Fixes

* **release:** use vX.Y.Z tag format (remove component) ([0a740bc](https://github.com/Roberdan/convergio-orchestrator/commit/0a740bc56b582b81884d155371e241ca6260473c))
* security audit — path traversal, state machine, input validation, credential hardening ([#2](https://github.com/Roberdan/convergio-orchestrator/issues/2)) ([7773e2b](https://github.com/Roberdan/convergio-orchestrator/commit/7773e2b7a44911d87d6a91c60b07f368b1879b5f))


### Documentation

* add .env.example with required environment variables ([#3](https://github.com/Roberdan/convergio-orchestrator/issues/3)) ([995edfa](https://github.com/Roberdan/convergio-orchestrator/commit/995edfa658067ea6ed427a91dde7e14ab9a9ff18))

## [0.1.2](https://github.com/Roberdan/convergio-orchestrator/compare/convergio-orchestrator-v0.1.1...convergio-orchestrator-v0.1.2) (2026-04-12)


### Documentation

* add .env.example with required environment variables ([#3](https://github.com/Roberdan/convergio-orchestrator/issues/3)) ([995edfa](https://github.com/Roberdan/convergio-orchestrator/commit/995edfa658067ea6ed427a91dde7e14ab9a9ff18))

## [0.1.1](https://github.com/Roberdan/convergio-orchestrator/compare/convergio-orchestrator-v0.1.0...convergio-orchestrator-v0.1.1) (2026-04-12)


### Features

* adapt convergio-orchestrator + evidence bundle for standalone repo ([20a35b9](https://github.com/Roberdan/convergio-orchestrator/commit/20a35b9da7347f35faf31eb4c2694c4f8979b7dc))


### Bug Fixes

* security audit — path traversal, state machine, input validation, credential hardening ([#2](https://github.com/Roberdan/convergio-orchestrator/issues/2)) ([7773e2b](https://github.com/Roberdan/convergio-orchestrator/commit/7773e2b7a44911d87d6a91c60b07f368b1879b5f))

## 0.1.0 (Initial Release)

### Features

- Initial extraction from convergio monorepo
