#ifndef LOCSIM_H
#define LOCSIM_H

#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

/// Đặt vị trí ảo. Lần đầu sẽ kết nối tới thiết bị (qua VPN loopback) bằng pairing file,
/// các lần sau chỉ cập nhật toạ độ. Trả về 0 nếu thành công.
/// Nếu lỗi, *err_out chứa chuỗi lỗi (giải phóng bằng locsim_free_string).
int32_t locsim_set(const char *pairing_file_path,
                   const char *device_ip,
                   double latitude,
                   double longitude,
                   char **err_out);

/// Tắt vị trí ảo và đóng kết nối. Trả về 0 nếu thành công.
int32_t locsim_clear(char **err_out);

/// 1 nếu đang có phiên giả lập đang chạy.
int32_t locsim_is_active(void);

void locsim_free_string(char *s);

#ifdef __cplusplus
}
#endif

#endif
