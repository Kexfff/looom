# Нативный установщик: VM-приёмка 2026-10-09

Результат: `looom install` создаёт с нуля GPT, EFI, Btrfs, пользователя,
recovery и первый read-only Arch/Plasma-релиз. Движок и мастер написаны на Rust
и входят в установленный `/usr/bin/looom`. Компилятор на целевой машине не нужен.
[Инструкция](../../installer.md), [профиль](../../../configs/installer/base.yaml).

## Проверенные машины и входы

Все сборки, форматирование, Cargo/Clippy, PAM и runtime-тесты выполнены **в VM**.
На рабочем компьютере использовались только редактор, Git, SSH и управление
тестовыми VM. Физический N100 в этом этапе не изменялся.

Основная development VM: QEMU/KVM, 6 vCPU, 8 GiB, UEFI, рабочий диск 60 GiB.
Установщик получил отдельный пустой virtio-диск 48 GiB с serial
`looom-installer-2026`. Первый полный опыт использовал `--no-nvram`, чтобы
сохранить выбор загрузки основной машины. Профиль, credential-хэши, recovery UKI,
GRUB saved/next и текущий релиз сравнивались с закрытым baseline.

После завершения диск отключён от development VM и подключён как единственный
диск к отдельной `Looom-installer-acceptance`: 4 vCPU, 4 GiB, новая OVMF NVRAM,
Secure Boot выключен. Переход `/dev/vdb` → `/dev/vda` подтвердил загрузку по UUID.
Использован fallback `EFI/BOOT/BOOTX64.EFI`, без заранее созданной UEFI-записи.

Зафиксированы официальный Arch snapshot `2026-10-07`, linux `7.2.9-arch1-1`,
653 подписанных пакета, Plasma Wayland, owner 1000:1000, wheel/Bash.
Первый релиз — `initial`. Приватные парольные fixtures и ключи остаются в VM,
не входят в этот журнал или Git. SHA-256 проверенного бинарника:

```text
0d824fc03b73eb14e351da7738f442ed28b6e2fc2d7c4925d96b95b3b9391eb7
```

## Проверки до записи и восстановление

Отрицательные проверки подтвердили отказ при неверной фразе стирания,
отсутствующем resume-журнале, изменённых serial/декларации, workspace-ссылке,
рабочем диске, рабочем Btrfs-разделе и чужом bind mount внутри target.
Проверка Btrfs специально учитывает анонимный `dev_t` монтирования.

Прерывания применены к настоящей установке, не только к модели журнала:

| Точка | Результат |
| --- | --- |
| `install-action-root-format` | `resume` отказался повторять форматирование; UUID не изменился |
| `install-root-format` | Завершённое форматирование пропущено |
| `install-action-subvolumes` | Повтор подготовил собственные подтомы и mounts |
| `install-packages` | Пакетный checkpoint сохранён, повторной разметки нет |
| `install-action-configure` | Конфигурация продолжена идемпотентно |
| `install-action-bootstrap` | Готовый bootstrap распознан |
| `build-packages` | Незавершённый первый private build удалён и пересобран; входы архивированы |
| `build` | Configured build завершён штатным recovery |
| `install-build` | Read-only checkpoint принят |
| `install-action-publish` | Публикация повторена без изменения saved default |
| `ready`, повторный `resume` | Все 12 этапов закончены; повтор не меняет пароли или boot choice |

Harness проверяет **точное сообщение требуемого failpoint**, а не любой ненулевой
exit. См. [отрицательные проверки](evidence/negatives.log),
[неопределённое форматирование](evidence/format-interruption.log),
[ранний build](evidence/unconfigured-build.log),
[configured build](evidence/build-interruption.log),
[завершение](evidence/installation-complete.log),
[сохранность development VM](evidence/primary-preserved.log).

Настоящий TTY-мастер проверен отдельно: отмена, несовпадающие root/user пароли,
отсутствие echo для обоих паролей и неизменность начала/конца пустого диска.
[Результат](evidence/wizard.log), [VM-only контроллер](evidence/wizard-test.py).

## Загрузка, рабочий стол и откат

1. `initial` загрузился через GRUB: корень read-only, kernel/modules согласованы,
   State/home/var writable, пакетный состав точный, аккаунты и службы исправны.
2. Вход через SDDM привёл к настоящей owner-сессии Plasma Wayland:
   kwin_wayland, plasmashell, PipeWire/WirePlumber. [Протокол](evidence/desktop.log),
   [скриншот](evidence/desktop.png).
3. Без подтверждения повторная загрузка вернула writable `@bootstrap`.
   Затем `try initial`, реальная загрузка, `verify` и `confirm` закрепили релиз.
4. Нативными `lock/build/publish/try` создан `installer-v2`: hostname
   `looom-updated`, добавлен jq. После загрузки проверены 655 пакетов и новый hostname.
5. `rollback initial` и перезагрузка вернули прежний hostname и отсутствие jq.
   Файлы `/home/owner` и `/etc/looom-local`, UID владельца и изменённый пароль сохранились.
   PAM login/sddm/sudo принимал правильный пароль и отвергал неправильный до и
   после отката. Исходный credential-хэш восстановлен точно; private fixture удалён.
6. Финальное состояние: `initial` confirmed, `installer-v2` тоже confirmed,
   GRUB saved=`looom-initial`, next пуст; failed units отсутствуют.
   GC preview сохраняет оба релиза.

[Первая загрузка](evidence/first-boot.log),
[возврат в recovery](evidence/recovery-fallback.log),
[сборка обновления](evidence/update-build.log),
[загрузка обновления](evidence/update-boot.log),
[откат и итоговая проверка](evidence/rollback-final.log).
`installation/result.json` — исторический протокол установки перед первой
загрузкой; текущее подтверждение показывает release registry / `looom status`.

## Live-среда и пределы

Официальный `archlinux-2026.10.01-x86_64.iso` перенесён в VM. SquashFS смонтирован
read-only, поверх него отдельный overlay; проверки запускаются через arch-chroot
в частном mount namespace. Готовый бинарник работает с библиотеками ISO.
Проверены UEFI/tools/locale preflight и отказ неверному JSON паролей до записи.
[Протокол](evidence/live-userspace.log).

Полная дополнительная установка из этого userspace прошла на другом пустом
48 GiB диске, включая default UEFI registration и завершённый повтор `resume`.
Проверены первый элемент BootOrder, partition GUID и путь GRUB. Контролируемый
повтор pending-этапа `boot` с `install-action-boot` подтвердил использование
единственной существующей UEFI-записи. После опыта её удалили и вернули исходный
BootOrder основной VM. [Полная установка](evidence/live-install.log),
[восстановление UEFI](evidence/live-boot-replay.log),
[VM-only контроллер](evidence/live-install.sh).
В chroot явно инициализирован ключ pacman, поскольку загрузочные
службы ISO там не запускаются. Это проверка userspace официального ISO на ядре
подготовленной VM; **загрузка самого официального ISO и его ядра не проверялась**.
Создание собственного Archiso и приёмка на N100 остаются следующим этапом.

Прерывания установщика здесь — программные failpoints. Hard power cut именно
этого установщика и физическое обесточивание накопителя не проверялись.
Результаты предыдущих испытаний менеджера не считаются испытаниями установщика.

Два растущих qcow2-образа заполнили host `/tmp` (tmpfs 16 GiB), и гипервизор
приостановил VM на I/O error. Завершённая acceptance VM штатно выключена; её
образ перенесён на постоянное хранилище. Основная VM возобновлена, live-установка
успешно закончилась. Для повторения хранить образы на постоянном диске.

## Найденные проблемы и воспроизведение

Первый прототип оставлял внутренние mounts после arch-chroot. Добавлены частный
mount namespace и проверяемая очистка собственных mounts перед завершением
target session. Предварительный опыт также оставил busy-ссылку Btrfs без видимого
mount: форматирование отказало, development VM перезагружена. Причина удержания
kernel-reference окончательно не установлена; `O_EXCL` добавлен как проверка
занятости диска. Финальная серия прерываний закончилась без оставленных mounts.

Первый harness принимал любой ненулевой exit за ожидаемый failpoint. Исправлен
на проверку точного сообщения; ранние ложноположительные попытки не включены
в результаты выше. Для Plasma кэш занимает около 1.8 GiB: общий `/run` с лимитом
20% RAM оказался слишком мал, мастер использует отдельный tmpfs 50% RAM.
Рекомендуются 8 GiB либо workspace на другом постоянном диске.

Rust toolchain находится в отдельном writable подтоме development VM.
С исходниками `/workspace`:

```sh
cargo fmt --check
cargo clippy --locked --all-targets -- -D warnings
LOOOM_TEST_VM=1 cargo test --release --locked -- --test-threads=1
cargo build --release --locked --bin looom --bin looom-vm-installer
```

Fmt, Clippy без предупреждений и полный набор тестов прошли:
5 config tests, isolated bootstrap и runtime suite.
[Протокол](evidence/rust-validation.log), [SHA исходников](evidence/source.sha256).
`looom-vm-installer` не входит в продукт и разрешён только для записанных
MAC/serial/размера VM. `prepare` создаёт закрытые fixtures; `negatives`,
`destructive`, `start`, `resume <failpoint>` и `report` проверяют установку.
`start` требует собственный отдельный виртуальный диск: основной диск не стирается.
Журналы экспортируются только после проверки на отсутствие паролей, credential
хэшей и private key markers. [Манифест](evidence/public-artifacts.sha256).
