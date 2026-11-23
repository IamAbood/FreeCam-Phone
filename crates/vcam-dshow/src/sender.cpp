// C ABI for the softcam sender side, called from Rust.
#include <softcamcore/SenderAPI.h>

extern "C" void* fc_create_camera(int width, int height, float fps)
{
    return softcam::sender::CreateCamera(width, height, fps);
}
extern "C" void fc_delete_camera(void* cam) { softcam::sender::DeleteCamera(cam); }
extern "C" void fc_send_frame(void* cam, const void* bits) { softcam::sender::SendFrame(cam, bits); }
extern "C" bool fc_is_connected(void* cam) { return softcam::sender::IsConnected(cam); }
