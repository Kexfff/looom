# Обратная связь установщика и Limine — 2026-10-10

Разработка, сборка, Clippy и все проверки выполняются только в выделенных VM.
На рабочем компьютере — редактирование, SSH, Git и управление собственными
тестовыми дисками/VM. ISO в этом этапе не собирается.

## Изменения

- Номер диска остаётся способом выбора. Старые разделы/сигнатуры допустимы,
  перед стиранием выводится предупреждение. Смонтированные разделы, swap,
  holders и работающая система блокируются с объяснением.
- Подтверждение — `YES`, без имени диска/серийного номера в набираемой фразе.
- Пользовательская копия `~/looom/base.yaml` и `base.lock` принадлежит UID/GID
  личного пользователя, 0644. Private протокол/credentials не копируются.
  Повтор конфигурирования сохраняет уже существующий пользовательский файл.
- Новый bootstrap и installer используют Limine 12.9.3-1 из снимка Arch
  2026-10-07. Постоянный выбор — конфиг, одноразовый — UEFI BLI.
- Старый GRUB backend и схема machine.json сохранены. `boot-migrate` добавляет
  Limine, сохраняет прежний default и оставляет старый загрузчик/fallback.
- Прямой recovery UKI доступен отдельной UEFI-записью. `recover` умеет
  восстановить даже пустое меню с recovery по умолчанию.

## Воспроизведение

Development VM `Cachyos`: UEFI, 6 vCPU/8 GiB, рабочий vda 60 GiB.
Дополнительный vdb 64 GiB без serial/WWN — копия нашего предыдущего установленного
тестового диска. На нём уже находятся разделы и система: проверяется именно
переустановка непустого диска, не только пустое устройство.

Toolchain chroot `/run/looom-toolchain`, исходники `/workspace`:

```sh
cargo fmt --check
cargo clippy --locked --all-targets -- -D warnings
cargo build --release --locked --bin looom
LOOOM_TEST_VM=1 cargo test --release --locked -- --test-threads=1
```

[Контроллер мастера](../../../scripts/test-installer-limine.py) запускается
в development VM и проверяет её MAC, виртуализацию и конкретный vdb. Отмена,
ошибочные номера, защита текущего root, ошибки паролей и подмена идентичности
проверяются до установки. Пароли — только из private fixture внутри VM.

```sh
python test-installer-limine.py negatives
python test-installer-limine.py install
```

Полный мастер вызывается с настоящим PTY. Контроллер устанавливает umask 0077,
проверяет отсутствие plaintext-паролей в выводе и выбирает диск по номеру.
Пакетный кэш прогрет архивами предыдущего теста, подписи/хэши проверяются заново.

Финальный бинарник:

```text
9b3602801ac3b06587f3639b30f84b43cb71b74c43c8d468d220ac00ff41057b
```

В ходе первой приёмки найден старый дефект recovery: pacstrap при umask 0077
оставляет заранее созданный `/etc` с mode 0700. D-Bus после смены UID не может
прочитать machine-id, сеть не поднимается. Read-only релиз при этом работает,
поскольку builder отдельно задаёт `/etc` mode 0755. В installer исправлены
права `/etc`; установка повторена. Дополнительная финальная сборка добавляет
восстановление повреждённого меню. Предварительные прогоны не используются
как доказательство полного успеха финального бинарника.

## Миграция прежней установки

Отдельная `Looom-installer-no-id`, 4 vCPU/4 GiB, ранее подтверждённый `initial`
на GRUB. Новый `limine-migration` собран и испытан через прежний загрузчик,
подтверждён, затем выбран Limine с тем же постоянным релизом.

Проверяются реальный `LoaderInfo = Limine 12.9.3`, здоровый root, сохранённые
GRUB EFI/fallback, повторный boot-migrate, одноразовый старт старого `initial`,
потребление BLI и возврат к `limine-migration` без подтверждения. Отдельно —
постоянный rollback к старому релизу и обратный rollback. Старые службы аккаунтов
совместимы с неизменённой схемой machine.json; для команд старого релиза
используется новый менеджер `/var/lib/looom/boot-manager`.

Также обновляется recovery: `bootstrap-update limine-migration limine-native`,
`bootstrap-try`, загрузка writable кандидата через Limine, `bootstrap-confirm`,
возврат в постоянный релиз и GC с защитой двух подтверждённых релизов/истории.
Миграционный прогон начат сборкой до финальных правок installer/recover;
его результаты не заявляют побитовую идентичность с финальным бинарником.

## Самостоятельная установка и загрузка

Установленный диск переносится в отдельную `Looom-feedback-limine`,
UEFI, 4 vCPU/4 GiB, MAC 52:54:00:10:10:11. Firmware variables у неё новые,
поэтому исходный BLI installer из development VM не переносится: первый старт
ведёт в recovery. Нормальный installer на одной машине сохраняет BLI-запрос
для первого read-only старта; его корректное значение проверяется до переноса.

[VM-контроллер загрузки](../../../scripts/test-limine-boot.py) проверяет реальную
идентичность Limine, SHA финального менеджера, сервисы, read-only root,
потребление one-shot и возможность UID 1000 прочитать/изменить пользовательскую
конфигурацию. Повреждение обоих меню проверяется отдельным direct recovery.

Финальная сборка прошла полную установку, completed resume, самостоятельный
старт recovery с работающей сетью/D-Bus/SDDM, две записи одного trial,
read-only `initial`, возврат в recovery без подтверждения, повторную пробу и
confirm. UID 1000 читает и изменяет свою YAML-копию; владелец lock также UID 1000.

Оба конфига Limine намеренно обнулены. Firmware BootNext выбрал прямой recovery
UKI, проверенный по фактическому BootCurrent/path. Сеть и необходимые службы
работают. Native `recover` пересобрал оба одинаковых меню с recovery-default;
после `try initial` система снова загрузила read-only релиз. Финальные verify,
confirm, GC и повторный boot-migrate прошли.

Первый контроллер direct recovery ошибочно ожидал успешный `status` при
намеренно пустом конфиге. `status` правильно отказал с `Limine default missing`;
контроллер исправлен, затем repair и последующая загрузка проверены полностью.
Это ошибка ожидания контроллера, не дополнительная правка финального бинарника.

Доказательства:

- [Финальная сборка/fmt/Clippy и 7 Rust-тестов](evidence/installer/final-repair-checks.log),
  [SHA исходников](evidence/installer/source.sha256).
- [Проверки мастера](evidence/installer/negatives.log),
  [полная финальная установка](evidence/installer/full-wizard-install.log),
  [completed resume](evidence/installer/completed-resume.log),
  [проверка первого BLI-запроса](evidence/installer/installer-trial.log).
- [Первый recovery](evidence/boot/recovery-first.log),
  [read-only проба](evidence/boot/initial-unconfirmed.log),
  [возврат без подтверждения](evidence/boot/unconfirmed-fallback.log),
  [confirm и повреждение меню](evidence/boot/confirm-and-damage.log),
  [прямой recovery и repair](evidence/boot/direct-recovery-repair.log),
  [итог](evidence/boot/final.log).
- [Переход GRUB → Limine](evidence/migration/migration-boot.log),
  [проба старого релиза](evidence/migration/old-release-trial.log),
  [возврат и rollback](evidence/migration/fallback-and-rollback.log),
  [постоянный откат к старому релизу](evidence/migration/rollback-legacy.log),
  [подготовка recovery](evidence/migration/bootstrap-update.log),
  [подтверждение recovery](evidence/migration/bootstrap-confirm.log),
  [GC после миграции](evidence/migration/final-gc.log).
- [Рабочая development VM сохранена](evidence/installer/primary-preserved.log).
  Её UEFI BootOrder и GRUB selection восстановлены; временные firmware entries
  удалены, BLI-проба тестового диска очищена перед переносом.

Аудит public evidence проводится внутри каждой исходной VM: реальные хэши
credentials и plaintext fixture не экспортируются. Проверены отсутствие
yescrypt-хэшей, приватных SSH-ключей и тестовых паролей в журналах.
[Аудит installer](evidence/installer/secret-audit.log),
[аудит boot](evidence/boot/secret-audit.log). Попытка переноса credential-хэшей
между VM для централизованного аудита отклонена автоматической проверкой;
она заменена аудитом на месте, без передачи секретного payload.

## Ограничения

UEFI/Secure Boot off, один диск целиком, без шифрования и dual boot.
Смонтированные чужие разделы не размонтируются автоматически. BLI-переменная
общая для прошивки; `--no-nvram` оставляет её неизменной и требует запуска
`try initial` из recovery. Отсутствуют watchdog, boot counting и автоматический
переход на GRUB при повреждённом EFI. До Calamares/live ISO — следующий этап.
