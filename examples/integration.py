"""
Integration Test - Rust (webrtc-rs) Version
Tests all features: DataChannel, LiDAR, Video, Audio
Robot IP: 10.2.80.101
"""

import asyncio
import time
import json
import numpy as np
from collections import deque
from unitree_webrtc_rs import UnitreeWebRTCConnection, WebRTCConnectionMethod

RTC_TOPIC = {
    "LOW_STATE": "rt/lf/lowstate",
    "SPORT_MOD": "rt/api/sport/request",
    "MOTION_SWITCHER": "rt/api/motion_switcher/request",
    "LF_SPORT_MOD_STATE": "rt/lf/sportmodestate",
}

SPORT_CMD = {
    # "Damp": 1001,
    "BalanceStand": 1002,
    "StandUp": 1004,
    "StandDown": 1005,
    "Hello": 1016,
}

class IntegrationTest:
    def __init__(self, ip="10.2.80.101"):
        self.ip = ip
        self.conn = None
        
        # Data collectors
        self.lowstate_received = False
        self.lowstate_data = None
        self.sportmode_state_data = None
        self.sportmode_responses = []
        
        self.lidar_frames = deque(maxlen=100)
        self.lidar_count = 0
        
        self.video_frames = deque(maxlen=100)
        self.video_count = 0
        
        self.audio_frames = deque(maxlen=100)
        self.audio_count = 0
        
    async def test_connect(self):
        """Test 1: Connection"""
        print("\n" + "="*70)
        print("TEST 1: Connection")
        print("="*70)
        
        self.conn = UnitreeWebRTCConnection(
            WebRTCConnectionMethod.LocalSTA, 
            ip=self.ip
        )
        
        start_time = time.time()
        await self.conn.connect()
        connect_time = time.time() - start_time
        
        print(f"✅ Connected in {connect_time:.2f}s")
        print(f"   IP: {self.ip}")
        
    async def test_datachannel_state(self):
        """Test 2: DataChannel - Get States"""
        print("\n" + "="*70)
        print("TEST 2: DataChannel - Get States")
        print("="*70)
        
        # Get LowState via pub/sub
        print("\n[LowState Request]")
        
        def lowstate_callback(message):
            self.lowstate_received = True
            self.lowstate_data = message['data']
        
        self.conn.datachannel.pub_sub.subscribe(RTC_TOPIC['LOW_STATE'], lowstate_callback)
        
        # Wait for data
        for _ in range(20):
            await asyncio.sleep(0.1)
            if self.lowstate_received:
                break
        
        if self.lowstate_data:
            print(f"✅ LowState received")
            print(f"   Type: {type(self.lowstate_data)}")
            print(f"   Keys: {list(self.lowstate_data.keys())[:10]}...")
            if 'imu_state' in self.lowstate_data:
                imu = self.lowstate_data['imu_state']
                print(f"   IMU quaternion: {imu.get('quaternion', [])[:4]}")
        else:
            print(f"❌ LowState not received")
        
        # Get SportModeState via pub/sub
        print("\n[SportModeState Request]")
        
        self.sportmode_received = False
        def sportmode_callback(message):
            self.sportmode_received = True
            if 'data' in message:
                self.sportmode_state_data = message['data']
            
        self.conn.datachannel.pub_sub.subscribe(RTC_TOPIC['LF_SPORT_MOD_STATE'], sportmode_callback)
        
        # Wait for data
        for _ in range(20):
            await asyncio.sleep(0.1)
            if self.sportmode_received:
                break
                
        if self.sportmode_state_data:
            print(f"✅ SportModeState received")
            print(f"   Type: {type(self.sportmode_state_data)}")
            print(f"   Keys: {list(self.sportmode_state_data.keys())[:10]}...")
        else:
            print(f"❌ SportModeState not received")
        
    async def test_datachannel_commands(self):
        """Test 3: DataChannel - Send Commands"""
        print("\n" + "="*70)
        print("TEST 3: DataChannel - SportMode Commands")
        print("="*70)
        
        # Command 1: StandUp
        print("\n[Command 1: StandUp]")
        response1 = await self.conn.datachannel.pub_sub.publish_request_new(
            RTC_TOPIC["SPORT_MOD"], 
            {"api_id": SPORT_CMD["StandUp"]}
        )
        self.sportmode_responses.append(("StandUp", response1))
        
        print(f"✅ StandUp response:")
        print(f"   Type: {type(response1)}")
        print(f"   Data: {response1}")
        
        await asyncio.sleep(1)
        
        # Command 2: Hello
        print("\n[Command 2: Hello]")
        response2 = await self.conn.datachannel.pub_sub.publish_request_new(
            RTC_TOPIC["SPORT_MOD"], 
            {"api_id": SPORT_CMD["Hello"]}
        )
        self.sportmode_responses.append(("Hello", response2))
        
        print(f"✅ Hello response:")
        print(f"   Type: {type(response2)}")
        print(f"   Data: {response2}")
        
    async def test_lidar(self):
        """Test 4: LiDAR Data"""
        print("\n" + "="*70)
        print("TEST 4: LiDAR Data Stream")
        print("="*70)
        
        # Disable traffic saving
        await self.conn.datachannel.disableTrafficSaving(True)
        
        # Set decoder type
        self.conn.datachannel.set_decoder(decoder_type='native')
        
        # Turn on LiDAR
        self.conn.datachannel.pub_sub.publish_without_callback("rt/utlidar/switch", "on")
        
        def lidar_callback(message):
            self.lidar_count += 1
            self.lidar_frames.append(time.time())
            
            if self.lidar_count == 1:
                data = message.get('data', {})
                points_data = data.get('data') if isinstance(data, dict) else data
                
                print(f"\n[First LiDAR Frame]")
                print(f"   Type: {type(points_data)}")
                print(f"   Shape: {points_data.shape if hasattr(points_data, 'shape') else 'N/A'}")
                print(f"   Dtype: {points_data.dtype if hasattr(points_data, 'dtype') else 'N/A'}")
                if hasattr(points_data, 'shape') and len(points_data.shape) >= 2:
                    print(f"   Points: {points_data.shape[0]}")
                    print(f"   Dims: {points_data.shape[1]}")
        
        self.conn.datachannel.pub_sub.subscribe("rt/utlidar/voxel_map_compressed", lidar_callback)
        
        print(f"✅ LiDAR enabled, collecting for 5 seconds...")
        await asyncio.sleep(5)
        
        lidar_fps = (len(self.lidar_frames) - 1) / (self.lidar_frames[-1] - self.lidar_frames[0]) if len(self.lidar_frames) >= 2 else 0
        
        print(f"\n✅ LiDAR Results:")
        print(f"   Total frames: {self.lidar_count}")
        print(f"   FPS: {lidar_fps:.2f}")
        
    async def test_video(self):
        """Test 5: Video Stream"""
        print("\n" + "="*70)
        print("TEST 5: Video Stream")
        print("="*70)
        
        def video_callback(frame):
            self.video_count += 1
            self.video_frames.append(time.time())
            
            # Print first frame details
            if self.video_count == 1:
                print(f"\n[First Video Frame]")
                print(f"   Type: {type(frame)}")
                print(f"   Shape: {frame.shape}")
                print(f"   Dtype: {frame.dtype}")
                print(f"   Size: {frame.nbytes / 1024:.1f} KB")
                print(f"   Min/Max: {frame.min()}/{frame.max()}")
        
        self.conn.video.on_frame(video_callback)
        self.conn.video.switchVideoChannel(True)
        
        print(f"✅ Video enabled, collecting for 5 seconds...")
        await asyncio.sleep(5)
        
        # Calculate FPS
        if len(self.video_frames) >= 2:
            time_span = self.video_frames[-1] - self.video_frames[0]
            fps = (len(self.video_frames) - 1) / time_span if time_span > 0 else 0
        else:
            fps = 0
        
        print(f"\n✅ Video Results:")
        print(f"   Total frames: {self.video_count}")
        print(f"   FPS: {fps:.2f}")
        
    async def test_audio(self):
        """Test 6: Audio Stream"""
        print("\n" + "="*70)
        print("TEST 6: Audio Stream")
        print("="*70)
        
        def audio_callback(audio_data):
            self.audio_count += 1
            self.audio_frames.append(time.time())
            
            # Print first frame details
            if self.audio_count == 1:
                print(f"\n[First Audio Frame]")
                print(f"   Type: {type(audio_data)}")
                print(f"   Shape: {audio_data.shape}")
                print(f"   Dtype: {audio_data.dtype}")
                print(f"   Samples: {len(audio_data)}")
                print(f"   Min/Max: {audio_data.min()}/{audio_data.max()}")
        
        self.conn.audio.on_audio(audio_callback)
        self.conn.audio.switchAudioChannel(True)
        
        print(f"✅ Audio enabled, collecting for 5 seconds...")
        await asyncio.sleep(5)
        
        # Calculate FPS
        if len(self.audio_frames) >= 2:
            time_span = self.audio_frames[-1] - self.audio_frames[0]
            fps = (len(self.audio_frames) - 1) / time_span if time_span > 0 else 0
        else:
            fps = 0
        
        print(f"\n✅ Audio Results:")
        print(f"   Total frames: {self.audio_count}")
        print(f"   FPS: {fps:.2f}")
        
    async def run_all_tests(self):
        """Run all integration tests"""
        print("\n" + "="*70)
        print("RUST INTEGRATION TEST (webrtc-rs)")
        print("="*70)
        print(f"Robot IP: {self.ip}")
        print(f"Start time: {time.strftime('%Y-%m-%d %H:%M:%S')}")
        
        try:
            await self.test_connect()
            await asyncio.sleep(1)
            
            await self.test_datachannel_state()
            await asyncio.sleep(1)
            
            await self.test_datachannel_commands()
            await asyncio.sleep(1)
            
            await self.test_lidar()
            await asyncio.sleep(1)
            
            await self.test_video()
            await asyncio.sleep(1)
            
            await self.test_audio()
            
            # Final summary
            print("\n" + "="*70)
            print("SUMMARY - Rust Version")
            print("="*70)
            print(f"✅ Connection: OK")
            print(f"✅ LowState: {len(self.lowstate_data)} keys" if self.lowstate_data else "❌")
            print(f"✅ SportModeState: {len(self.sportmode_state_data)} keys" if self.sportmode_state_data else "❌")
            print(f"✅ Commands sent: {len(self.sportmode_responses)}")
            
            lidar_fps = (len(self.lidar_frames) - 1) / (self.lidar_frames[-1] - self.lidar_frames[0]) if len(self.lidar_frames) >= 2 else 0
            video_fps = (len(self.video_frames) - 1) / (self.video_frames[-1] - self.video_frames[0]) if len(self.video_frames) >= 2 else 0
            audio_fps = (len(self.audio_frames) - 1) / (self.audio_frames[-1] - self.audio_frames[0]) if len(self.audio_frames) >= 2 else 0
            
            print(f"✅ LiDAR: {self.lidar_count} frames @ {lidar_fps:.1f} FPS")
            print(f"✅ Video: {self.video_count} frames @ {video_fps:.1f} FPS")
            print(f"✅ Audio: {self.audio_count} frames @ {audio_fps:.1f} FPS")
            print("="*70)
            
        except Exception as e:
            print(f"\n❌ Error: {e}")
            import traceback
            traceback.print_exc()


async def main():
    test = IntegrationTest(ip="10.2.80.114")
    await test.run_all_tests()


if __name__ == "__main__":
    asyncio.run(main())
