import asyncio
from unitree_webrtc_rs import UnitreeWebRTCConnection, WebRTCConnectionMethod

async def main():
    conn = UnitreeWebRTCConnection(WebRTCConnectionMethod.LocalSTA, ip="10.2.80.101")
    await conn.connect()
    print("✅ Connected")
    
    await conn.datachannel.disableTrafficSaving(True)
    conn.datachannel.set_decoder(decoder_type='native')
    conn.datachannel.pub_sub.publish_without_callback("rt/utlidar/switch", "on")
    print("✅ LiDAR enabled")
    
    frame_count = 0
    
    def lidar_callback(message):
        nonlocal frame_count
        frame_count += 1
        if frame_count == 1:
            print(f"\n[Rust] First LiDAR frame:")
            print(f"  Message type: {type(message)}")
            data = message.get('data', {})
            print(f"  Data type: {type(data)}")
            if isinstance(data, dict):
                points_data = data.get('data')
                print(f"  Points data exists: {points_data is not None}")
                if points_data is not None:
                    print(f"  Points type: {type(points_data)}")
                    if hasattr(points_data, 'shape'):
                        print(f"  Points shape: {points_data.shape}")
    
    conn.datachannel.pub_sub.subscribe("rt/utlidar/voxel_map_compressed", lidar_callback)
    print("✅ Subscribed to LiDAR")
    
    await asyncio.sleep(5)
    print(f"\n✅ Received {frame_count} LiDAR frames")

if __name__ == "__main__":
    asyncio.run(main())
