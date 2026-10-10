# Загрузка looom через Limine

Новые установки используют Limine из того же закреплённого снимка Arch,
что и ядро/пакеты. В снимке 2026-10-07 это **12.9.3-1**. looom проверяет,
что пакет поддерживает `LoaderEntryOneShot`; старые версии без BLI не подходят.
[Документация этой версии](https://github.com/limine-bootloader/limine/blob/v12.9.3/CONFIG.md),
[потребление одноразового выбора](https://github.com/limine-bootloader/limine/blob/v12.9.3/common/lib/bli.c).

## Обычная работа

```sh
sudo looom build ~/looom/base.yaml next
sudo looom publish next
sudo looom try next
sudo reboot
# Проверить рабочий стол, затем:
sudo looom verify
sudo looom confirm
# Возврат к ранее подтверждённому релизу:
sudo looom rollback initial
sudo reboot
```

После изменения пакетов сначала выполнять `sudo looom lock ~/looom/base.yaml`.
`build` и `publish` не меняют постоянный выбор. `try` действует на следующую
загрузку; без `confirm` последующая перезагрузка возвращает прежний default.
Зависание требует перезагрузки пользователем; автоматического watchdog пока нет.

На ESP:

- `/EFI/looom/liminex64.efi` — закреплённый EFI-загрузчик;
- `/EFI/looom/limine.conf` — сгенерированное меню и `default_entry`;
- `/EFI/BOOT/BOOTX64.EFI` и `limine.conf` — fallback новой установки;
- `/EFI/Linux/looom-<id>.efi` — UKI опубликованного релиза;
- `/EFI/Linux/looom-bootstrap.efi` — writable recovery.

`remember_last_entry: no` сохраняет постоянный выбор при пробной загрузке.
Однократный выбор хранится в EFI-переменной BLI `LoaderEntryOneShot`, а не
в изменённом корне. Limine удаляет её до запуска ядра. Переменная общая для
UEFI-машины: looom блокирует неизвестный запрос другого загрузчика.

В State `/var/lib/looom/bootloader.json` указан backend. Отсутствие файла означает
исторический GRUB. Схема `machine.json` не меняется, поэтому старые службы
аккаунтов продолжают читать профиль при откате. GRUB-пакет пока сохранён в
closure для совместимости; новые установки загружаются через Limine.

## Переход существующей GRUB-установки

Не нужно форматировать диск. Сначала получить новый бинарник из проверенной
сборки или собрать репозиторий. Ниже `./looom-new` — именно новый бинарник;
старый установленный менеджер ещё не знает команды `boot-migrate`.

Если раньше конфигурация оставалась только в закрытом протоколе:

```sh
mkdir -p ~/looom
sudo cp /var/lib/looom/installation/base.yaml ~/looom/base.yaml
sudo chown "$(id -u):$(id -g)" ~/looom/base.yaml
```

Подготовить новый релиз, содержащий новый менеджер и пакет Limine:

```sh
sudo ./looom-new lock ~/looom/base.yaml
sudo ./looom-new build ~/looom/base.yaml limine-ready
sudo ./looom-new publish limine-ready
sudo ./looom-new try limine-ready
sudo reboot
# Этот запуск ещё через GRUB:
sudo looom verify
sudo looom confirm
sudo looom boot-migrate
sudo looom boot-recovery
sudo reboot
sudo looom verify
sudo looom status
```

`boot-migrate` требует завершить ожидающую GRUB-пробу и иметь подтверждённый
постоянный релиз. Он сохраняет прежний EFI-образ GRUB, его меню и fallback,
копирует Limine, сохраняет тот же default и регистрирует новую UEFI-запись.
Повторный вызов на Limine-машине ничего не меняет. Пакеты на работающем
read-only корне не устанавливаются через pacman.

Новый менеджер дополнительно сохраняется в `/var/lib/looom/boot-manager`.
После отката к старому релизу или запуска старого recovery для управления
**Limine** применять этот бинарник через sudo. Старый `/usr/bin/looom`
продолжает управлять только историческим GRUB:

```sh
sudo /var/lib/looom/boot-manager status
sudo /var/lib/looom/boot-manager try limine-ready
```

Для обновления recovery до нового менеджера можно использовать уже существующий
цикл `bootstrap-update`, `bootstrap-try`, reboot и `bootstrap-confirm`.

## Повреждённое меню

Обычный installer и `looom boot-recovery` создают UEFI-запись
`looom-recovery-<первые 8 символов root UUID>`, которая напрямую запускает recovery
UKI. Выбрать её в меню прошивки; конфиг Limine для этого не нужен.
В recovery новый менеджер восстанавливает меню командой `looom recover`.
Если постоянный выбор потерян или больше не указывает на опубликованный релиз,
меню восстанавливается с recovery по умолчанию. Затем явно выполнить
`looom try <рабочий-id>`, reboot и `looom confirm`.
Старому recovery после миграции нужен `/var/lib/looom/boot-manager recover`.

При повреждении самого UKI, ESP или прошивки потребуется live-система.
Встроенного в Limine аварийного конфига, автоматического переключения на GRUB
и boot counting в этой версии looom нет.
