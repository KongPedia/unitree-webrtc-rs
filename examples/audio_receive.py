import argparse
import asyncio
import sys
import time
from queue import Queue
import numpy as np

try:
    import pyaudio
    PYAUDIO_AVAILABLE = True
except ImportError:
    PYAUDIO_AVAILABLE = False
    print("Warning: pyaudio not available, audio playback disabled")

from unitree_webrtc_rs import UnitreeWebRTCConnection, WebRTCConnectionMethod

frame_queue = Queue()
frame_count = 0
test_start = time.time()

def audio_callback(audio_data):
    global frame_count
    frame_count += 1
    
    # Put frame in queue for main thread processing
    frame_queue.put((audio_data.copy(), time.time()))

async def main():
    parser = argparse.ArgumentParser(description='Audio stream test')
    parser.add_argument('--ip', type=str, default='10.2.80.101', help='Robot IP address')
    args = parser.parse_args()

    print(f"Connecting to robot at {args.ip}...")
    conn = UnitreeWebRTCConnection(WebRTCConnectionMethod.LocalSTA, ip=args.ip)
    
    await conn.connect()
    print("Connected!")
    
    # Register audio callback
    conn.audio.on_audio(audio_callback)
    
    # Switch audio channel on
    conn.audio.switchAudioChannel(True)
    
    print("Audio streaming started. Analyzing frames for 10 seconds...")
    
    # Setup PyAudio if available
    p = None
    stream = None
    if PYAUDIO_AVAILABLE:
        p = pyaudio.PyAudio()
        stream = p.open(
            format=pyaudio.paInt16,
            channels=2,
            rate=48000,
            output=True,
            frames_per_buffer=1920
        )
        print("PyAudio stream opened for playback")
    
    fps_window_start = time.time()
    fps_window_frames = 0
    
    try:
        while True:
            if not frame_queue.empty():
                audio_data, recv_time = frame_queue.get()
                fps_window_frames += 1
                
                # Print frame info
                if frame_count <= 5 or frame_count % 50 == 0:
                    print(f"\n=== Audio Frame {frame_count} ===")
                    print(f"  dtype: {audio_data.dtype}")
                    print(f"  shape: {audio_data.shape}")
                    print(f"  size: {audio_data.size}")
                    print(f"  samples: {audio_data.size}")
                    print(f"  min: {audio_data.min()}, max: {audio_data.max()}")
                
                # Calculate FPS every second
                elapsed = time.time() - fps_window_start
                if elapsed >= 1.0:
                    fps = fps_window_frames / elapsed
                    print(f"Audio Frame {frame_count}: FPS={fps:.2f} | Samples={audio_data.size}")
                    fps_window_start = time.time()
                    fps_window_frames = 0
                
                # Play audio if PyAudio is available
                if stream is not None:
                    stream.write(audio_data.tobytes())
            else:
                await asyncio.sleep(0.001)
            
            # Auto-stop after 10 seconds
            if time.time() - test_start > 10.0:
                print("\n10 second test completed.")
                break
                
    except KeyboardInterrupt:
        print("\nInterrupted by user")
    finally:
        total_time = time.time() - test_start
        avg_fps = frame_count / total_time if total_time > 0 else 0
        print(f"\nTotal frames: {frame_count}, Time: {total_time:.2f}s, Avg FPS: {avg_fps:.2f}")
        
        if stream:
            stream.stop_stream()
            stream.close()
        if p:
            p.terminate()

if __name__ == "__main__":
    asyncio.run(main())
