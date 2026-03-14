import asyncio
from unitree_webrtc_rs import UnitreeWebRTCConnection, WebRTCConnectionMethod

RTC_TOPIC = {
    "LOW_STATE": "rt/lf/lowstate",
}

async def main():
    conn = UnitreeWebRTCConnection(WebRTCConnectionMethod.LocalSTA, ip="10.2.80.101")
    await conn.connect()
    print("✅ Connected")
    
    frame_count = 0
    
    def lowstate_callback(message):
        nonlocal frame_count
        frame_count += 1
        if frame_count == 1:
            print(f"\n[Rust] First LowState frame:")
            print(f"  Type: {type(message)}")
            print(f"  Keys: {list(message.keys())}")
            data = message.get('data', {})
            print(f"  Data type: {type(data)}")
            print(f"  Data keys: {list(data.keys())[:10] if isinstance(data, dict) else 'not dict'}")
            print(f"  IMU state exists: {'imu_state' in data if isinstance(data, dict) else False}")
    
    conn.datachannel.pub_sub.subscribe(RTC_TOPIC['LOW_STATE'], lowstate_callback)
    print("✅ Subscribed to LOW_STATE")
    
    await asyncio.sleep(3)
    print(f"\n✅ Received {frame_count} LowState frames")

if __name__ == "__main__":
    asyncio.run(main())
