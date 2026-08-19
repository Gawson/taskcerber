// Nagłówki, z których bindgen generuje wiązania do epdiy.
//
// Świadomie BEZ `bindings_module` w Cargo.toml — dzięki temu symbole epdiy trafiają
// do korzenia crate'a esp_idf_sys i typy takie jak i2c_master_bus_handle_t są tożsame
// z tymi, których używamy po stronie Rusta przy tworzeniu magistrali.
#pragma once

#include "epdiy.h"
#include "epd_highlevel.h"
#include "epd_board.h"
#include "epd_display.h"
#include "epd_init_config.h"
