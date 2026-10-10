# Пользовательские приложения — 2026-10-10

ISO отложен по решению пользователя. Реализован Rust-слой приложений:
[контракт и команды](../../apps.md). Сборка, Clippy, Rust-тесты, установки,
запуски GUI и откаты выполнялись исключительно в VM. На рабочем компьютере —
редактирование, SSH, Git и управление выделенными виртуальными дисками.

## Что реализовано

- Отдельный пользовательский `apps.yaml`, строгая схема 1, отсутствие `sudo`
  для менеджера приложений. Ручные приложения не удаляются при `apply`.
- Пользовательский Flathub, Flatpak install/status/update.
- Лениво создаваемый `looom-arch` в rootless Podman/Distrobox, декларативные
  пакеты, экспорт desktop-приложений, явный полный `pacman -Syu` при update.
- `.sh` запускается целиком как root контейнера; `pacman`, `sudo pacman` и
  абсолютный `/usr/bin/pacman` изменяют контейнер, не host root.
- AppImage из HTTPS или локального файла: SHA256, проверка формата, атомарная
  активация, desktop launcher через Rust, повторная проверка перед запуском.
- Инфраструктура входит в новые базовые релизы и установщик; `apps.yaml`
  принадлежит пользователю. `/etc/subuid` и `/etc/subgid` одинаковы между
  релизами: один управляемый пользователь, диапазон `100000:65536`.
- Успешный `applied.json` содержит декларацию, Flatpak commits и ID OCI-образа.
  Автоматического удаления/GC пользовательских приложений нет.

## Машины и сборка

Development: `Cachyos`, MAC `52:54:00:7b:23:63`, UEFI, 6 vCPU/8 GiB,
рабочий vda 60 GiB. Rust 1.99 в отдельном toolchain-подтоме; исходники
`/var/lib/looom/dev/src` доступны внутри chroot как `/workspace`.

```sh
cargo fmt
cargo clippy --all-targets -- -D warnings
cargo build --release
LOOOM_TEST_VM=1 cargo test -- --test-threads=1
```

Девять интеграционных Rust-тестов прошли: приложения — 2, конфигурация — 5,
bootstrap — 1, runtime/credentials/publication/GC — 1. После окончательной
доработки повторены Clippy, release build и оба теста приложений.
[Полный прогон](primary/rust-suite.log), [финальная сборка](primary/rust-final.log).
[SHA исходников](source.sha256) получены непосредственно в VM.

Финальный бинарник, проверенный установленным менеджером и новым установщиком:

```text
ab2600391530fb5927075f714ce0e82b4c76e40f56593cf2685efe4205dc3152
```

Desktop acceptance: `Looom-feedback-limine`, MAC `52:54:00:10:10:11`,
UEFI/Limine 12.9.3, 4 vCPU/4 GiB, vda 64 GiB, пользователь `kexfff` UID/GID 1000.
Из существующего `initial` собраны `apps-a` и `apps-b` с инфраструктурой.
Arch snapshot `2026-10-07`, 679 подписанных пакетов. `apps-a` содержит
предварительный вариант receipt, `apps-b` — окончательный бинарник выше.
Оба загружены и подтверждены после `looom verify`.

```sh
# Внутри desktop VM, новый бинарник из development VM:
/var/lib/looom/dev/apps/looom lock /var/lib/looom/dev/apps/base.yaml
/var/lib/looom/dev/apps/looom build /var/lib/looom/dev/apps/base.yaml apps-a
/var/lib/looom/dev/apps/looom publish apps-a
/var/lib/looom/dev/apps/looom try apps-a
# Перезагрузка, verify, confirm; для apps-b используется looom-final и новый lock.
```

[Сборка A](desktop/build-apps-a.log), [сборка B](desktop/build-apps-b.log),
[загрузка A](desktop/boot-apps-a.log), [загрузка B](desktop/boot-apps-b.log).

## Приложения и ошибки

[Контроллер](../../../scripts/test-user-apps.py) запускается только внутри
указанной desktop VM обычным пользователем. Фикстуры лежат в
`~/looom/acceptance`, YAML — `~/looom/apps.yaml`.

Проверены реальные источники: Flathub `org.gnome.Calculator` 51.0,
Arch `tree` 2.3.2 и `xterm` 411, официальный AppImage appimagetool 1.9.1
по HTTPS и тот же файл через `source`. Digest AppImage закреплён в контроллере;
OCI manifest digests, runtime Flatpak и applied receipt сохранены в
[resolved-state.log](desktop/resolved-state.log).

```sh
python3 ~/looom/acceptance/test-user-apps.py install
looom apps update
looom apps apply
python3 ~/looom/acceptance/test-user-apps.py negative
python3 ~/looom/acceptance/test-user-apps.py parallel
python3 ~/looom/acceptance/test-user-apps.py snapshot
```

Проверены повторное применение, сохранение ручных/ранее объявленных установок,
экспорт xterm и launcher AppImage, literal argv с пробелами и shell-метасимволами,
cwd/сопутствующий файл `.sh`, установка `jq` и `bc` скриптом. После `.sh`
`looom verify` подтвердил точный неизменённый системный inventory.

Неверный SHA, неверный формат и изменённый AppImage отказывают до исполнения/
активации. Не совпавший образ и другой URL Flathub не принимаются. Старый receipt
и рабочий launcher сохраняются. Долгая `apps exec` не держит management lock.
[Первичная установка](desktop/user-install.log),
[финальные проверки](desktop/user-final.log),
[система после скрипта](desktop/host-preserved.log).

## Настоящие окна и пользовательские настройки

Для виртуального GUI использован штатный пользовательский D-Bus, виртуальный
KWin и Xwayland. Вспомогательный `xorg-xwininfo` установлен **в Arch-контейнер**.
[GUI-контроллер](../../../scripts/test-apps-gui-session.py) запускает xterm
через экспортированный `.desktop` и Flatpak Calculator, затем проверяет оба
окна в дереве Xwayland.

```sh
DBUS_SESSION_BUS_ADDRESS=unix:path=/run/user/1000/bus \
XDG_CURRENT_DESKTOP=KDE XDG_SESSION_TYPE=wayland \
timeout 55 kwin_wayland --virtual --xwayland --no-lockscreen \
  --no-global-shortcuts --socket looom-acceptance \
  --exit-with-session=/home/kexfff/looom/acceptance/gui-session.py
```

Предварительный запуск через отдельный `dbus-run-session` получил cgroup
Permission denied для Podman. Контроллер исправлен на штатный user bus;
Rust-бинарник не менялся. [Предварительный протокол](desktop/gui-private-bus.log),
[штатный bus](desktop/gui-user-bus.log),
[финальный запуск desktop launcher](desktop/gui-desktop.log).
Виртуальный GPU сообщает Vulkan/DRM-предупреждения, GUI запускается с fallback.
Аппаратное ускорение N100 и native Wayland приложений в этом прогоне не заявляются.

Настройка Calculator `org.gnome.calculator show-thousands` выставлена в `true`.
Выполнены два цикла `apps-b → apps-a → apps-b`. В первом также выполнен
`looom gc --keep 2 --apply`: старый системный `initial` удалён, пользовательский
слой сохранён. Во втором отдельно подтверждена сохранность настоящей настройки
Flatpak. Сравниваются YAML, receipt, active.json/launcher, результат скрипта,
container ID, Flatpak commit и значение настройки. Приложения запускаются в
обоих релизах. Файлы из HOME и контейнерное хранилище не пересоздаются.

[Откат](desktop/rollback-apps-a.log), [возврат и GC](desktop/return-apps-b-gc.log),
[настройка до отката](desktop/preferences-before.log),
[настройка после отката](desktop/preferences-rollback.log),
[настройка после возврата](desktop/preferences-return.log).

## Полный свежий установщик

Development VM получила новый **расходный** qcow2 vdb 64 GiB без serial/WWN.
В VM на нём создан GPT/ext4-раздел, затем полный мастер подтвердил стирание
через `YES`. Рабочий vda не затрагивался. Контроллер использует прежнюю
private password fixture только внутри этой VM и проверяет отсутствие её
значений в публичном протоколе.

```sh
python3 /var/lib/looom/dev/src/scripts/test-installer-apps.py install
python3 /var/lib/looom/dev/src/scripts/test-installer-apps.py inspect
python3 /var/lib/looom/dev/src/scripts/test-installer-apps.py cleanup
```

[Контроллер](../../../scripts/test-installer-apps.py) повторно использует
драйвер мастера с защищённым `__main__`. После полной установки через mount
проверены: readonly `@root-initial`, точный SHA менеджера, инфраструктура,
стабильные subordinate maps и точный `~/looom/apps.yaml`, UID/GID 1000, mode 0644.
Это проверка полной установки и содержимого диска; самостоятельная загрузка
этого нового расходного диска не выполнялась. Реальные загрузки/приложения
проверены на desktop VM выше.

[Результат](primary/fresh-installer.log),
[полный протокол мастера](primary/full-wizard-install.log),
[проверка и cleanup](primary/installer-cleanup.log).
Созданные только для vdb UEFI-записи и его BLI удалены, диск отключён.
Development VM оставлена на `rust-gc-ready`; toolchain mounts сняты.
Desktop VM оставлена на подтверждённом `apps-b`. На ней около 50 GiB свободно.
Тестовый qcow2 сохраняется в private `.local/installer-access/vm` с ограниченными
правами, в Git не включён.

Публичные логи проверены **в каждой исходной VM** по её локальным credentials
перед включением в Git; credentials между VM или на рабочий компьютер не
передавались. [Primary audit](primary/public-audit.log),
[desktop audit](desktop/public-audit.log). Private fixtures/installer workspaces
не включены. SHA всех публикуемых файлов — [public-artifacts.sha256](public-artifacts.sha256).
