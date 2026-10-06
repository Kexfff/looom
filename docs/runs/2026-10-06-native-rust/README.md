# Журнал полного переноса текущего менеджера на Rust

Дата: 2026-10-06. Цель — завершить Rust runtime текущего согласованного MVP
и подготовить будущий пилот на реальном компьютере. Все компиляции,
форматирование, Clippy и тесты выполняются **только в разрешённой VM**.
На хосте — редактирование, SSH-доставка исходников, просмотр экрана VM.
[Текущий интерфейс](../../native-rust.md),
[инструкция пилота](../../physical-pilot.md).

## Что изменилось

Установленный `/usr/bin/looom` теперь сам выполняет пакетный lock/plan,
private build, конфигурацию, создание UKI, финализацию, восстановление,
публикацию, пробную загрузку, подтверждение, откат и GC. Credentials и ранние
systemd account units тоже используют этот Rust-бинарник.
Python/Bash backend проекта в новые корни не устанавливается и не вызывается.
Штатные Arch-инструменты используются как системные зависимости.

Добавлены защищённый machine profile без VM-привязки, подключение готового
Arch через `bootstrap`, отдельная регистрация UEFI через `boot-entry`,
portable initramfs без autodetect, password-required sudo по умолчанию и
версионированные registry generations с атомарным указателем.
Входная декларация и бинарник заморожены; SHA бинарника участвует в lock.

## Среда и порядок проверки

VM: QEMU/KVM, UEFI/OVMF, 6 vCPU, 8 GiB RAM, диск 60 GiB, FAT ESP 2 GiB,
Btrfs на `/dev/vda2`; Secure Boot и шифрование выключены.
Компилятор Rust/Cargo 1.99.0 расположен в отдельном writable
`@toolchain-20261006`, source — `/var/lib/looom/dev/src`.
Действующий read-only корень не использовался для установки компилятора.

Последовательность фактических операций:

1. Регистрация существующей VM нативным `init`, сохранение её общих подтомов
   и credentials. Legacy flock переведён в 0600 без замены inode.
2. Native console `rust-a`: linux, tree, файл `/etc/looom-native.conf`,
   mask bluetooth. Созданы private корень, UKI и read-only снимок, затем
   публикация, настоящая загрузка, verify и confirm.
3. Временный случайный пароль пользователя установлен нативным Rust-компонентом;
   оригинал остаётся только в protected guest state.
4. Native console `rust-b`: linux-lts, без tree/файла/mask. Прерывание
   `LOOOM_FAIL_AFTER=snapshot` оставило read-only корень без release metadata.
   `recover` из установленного Rust A завершил его замороженным Rust B бинарником.
5. Публикация, реальная загрузка B и confirm. Затем `rollback rust-a` и
   реальная загрузка A: ядро, пакет, конфиг и unit вернулись; новый пароль,
   домашний и локальный маркеры сохранились. После этого снова загружен B.
6. Native bootstrap протестирован на writable снимке исходного Arch,
   отдельных временных home/var/state и FAT-образе 512 MiB. Проверены импорт
   credentials и существующих локальных настроек, собственный UKI/GRUB,
   машина с password-required sudo и отказ повторного подключения.
   Реальные ESP и NVRAM в fixture не изменялись.
7. Нативный `boot-entry` зарегистрировал настоящую запись VM Boot0005 looom;
   при последующей загрузке подтверждено BootCurrent=0005.
8. Native `rust-desktop` собран, опубликован и реально загружен. PAM SDDM
   принимает временный пароль и отвергает неверный. Rust VM harness вводит
   пароль через guest uinput без передачи секрета хосту. Подтверждена настоящая
   Wayland-сессия codex и активные Plasma/звуковые службы.
9. Исправлен импорт уже существующего `/etc/looom-local`, добавлена проверка
   его содержимого. Повторены Rust suite, Clippy, release build; собран
   окончательный `rust-final` с тем же desktop-профилем.
10. `rust-final` реально загружен через Boot0005, проверен установленным
    `looom verify`, принят в SDDM и проверен отдельной Rust GUI-приёмкой:
    local Wayland, Plasma/kwin и PipeWire/WirePlumber активны. Релиз подтверждён.
    Исходный пароль восстановлен, временный protected test state удалён,
    runtime credentials согласованы. Домашний и локальный маркеры сохранились.
    Повторный plan содержит нулевые package/file/unit changes.

Итог: running/saved — `rust-final`, next пуст; ядро `7.2.8-arch1-2`,
651 пакет, системных и пользовательских failed units нет.
SHA-256 установленного и экспортированного менеджера:
`ad1b202065e3c46559bf917d02480572a612e972616c3d049f1278634512b467`.
Console B использует `6.18.54-2-lts`; подтверждённые A/B и bootstrap сохранены.

При первом bootstrap-тесте вложенное тестовое монтирование мешало cleanup.
Fixture оставлен для явного размонтирования, затем удалены только его четыре
уникальных подтома. Реализация исправлена: bootstrap не оставляет top-level
mount, test mount получает private propagation. Повторные проверки прошли.
Рекурсивное удаление занятой точки монтирования не применялось.

## Покрытие

| Проверка | Результат |
| --- | --- |
| Typed YAML: положительный пример и 17 отрицательных вариантов | PASS |
| Симлинк файла/родителя; переносимое имя/UID/GID | PASS |
| Credentials: owner/mode/тип/ACL/default ACL/отсутствие/повреждение | PASS |
| Непривилегированное чтение credentials и runtime shadow | Запрещено |
| Сбой после credential commit, генерация и 8 конкурентных смен | PASS |
| PAM login и SDDM: правильный и неверный пароль | PASS |
| Установленный looom-password и protected stdin | PASS |
| FAT 128 MiB: реальное ENOSPC и прерывание после UKI | PASS |
| Recover/menu: незавершённая публикация, сохранение saved choice, повтор | PASS |
| GC: dry-run/apply собственной временной записи Btrfs | PASS |
| Изолированный prepared-Arch bootstrap и отказ повтора | PASS |
| Установленный CLI: stale recipe lock/corrupt archive digest | Отказ до frozen inputs |
| Configured build: снимок без метаданных, recover | PASS |
| Настоящий цикл Rust A → B → A и сохранение данных/пароля | PASS |
| UEFI Boot0005, KDE Wayland, PipeWire/WirePlumber | PASS |

Использованы пять configuration tests, один составной runtime test и один
bootstrap integration test. Команда запуска в compiler chroot:

```bash
cd /workspace
cargo fmt
LOOOM_TEST_VM=1 cargo test --locked -- --test-threads=1
cargo clippy --locked --all-targets -- -D warnings
cargo build --locked --release
```

Это функциональные и выбранные fault tests. Полный power-cut при FAT-записи,
полный Btrfs ENOSPC, обновление всего snapshot archive, независимый security
аудит и физические драйверы остаются отдельными проверками. Shared `/var`
не получает автоматической миграции данных при откате базы.

Публичные доказательства сохранены после просмотра содержимого, проверки
JSON-схем и поиска credential values по явному списку файлов:

- [Rust tests](evidence/cargo-tests.log), [Clippy](evidence/cargo-clippy.log),
  [release build](evidence/cargo-build.log), [fmt check](evidence/cargo-fmt.log).
- [Восстановление B](evidence/native-recover-b.log),
  [откат на A](evidence/native-rollback-a.log),
  [сохранение пароля](evidence/native-password-rollback.log).
- [Отрицательные входы](evidence/native-input-failures.log),
  [установленный password CLI](evidence/native-password-cli.log).
- [Итоговый verify](evidence/native-boot-final.log),
  [графическая сессия](evidence/native-final-desktop.log),
  [SDDM PAM](evidence/native-final-pam.log),
  [восстановление пароля](evidence/native-password-restored.log).
- [Итоговый JSON и SHA исходников](evidence/native-final-state.json),
  [нулевой plan](evidence/native-current-plan.json),
  [контрольные суммы публичных файлов](evidence/public-artifacts.sha256).
- Метаданные и фактические frozen locks:
  [A](../../../manifests/native/rust-a.json),
  [B](../../../manifests/native/rust-b.json),
  [final](../../../manifests/native/rust-final.json).

Пароли, password hashes, private keys, bootstrap/toolchain filesystem и
защищённые backups не экспортированы. Полные package-install logs остались
в защищённой VM; среди штатных сообщений были firmware warnings широкого
initramfs и ранний fontconfig hook с отсутствующим vercmp в новом корне.
Финальные inventory, загрузка, графическая сессия и тесты прошли.
Готовый проверенный бинарник на хосте находится в ignored `.local/artifacts/`;
его checksum входит в публичный список. Frozen inputs каждой сборки сохраняются
на VM; sample locks A/B отдельно обновлены установленным финальным бинарником.
