"""E1a live verification; deterministic agents, local owned environment only."""
import argparse
import asyncio
import json
from pathlib import Path
import statistics
import struct
import subprocess
import sys
import time

from environment import clean_env
from faultctl import control
from harness import WebSocket, api, endpoint, frame_bytes, load_state, read_frame
from register_mock import register
from smoke import Smoke


def stats(values):
    ordered = sorted(values)
    return {'n': len(values), 'min_ms': min(values), 'median_ms': statistics.median(values),
            'p95_ms': ordered[int((len(values)-1)*.95)], 'max_ms': max(values)}


class Verify(Smoke):
    def __init__(self, state, evidence):
        super().__init__(state)
        self.evidence = Path(evidence)

    def save(self, name, result):
        (self.evidence / (name + '.json')).write_text(json.dumps(result, indent=2) + '\n')
        print(name + ': ' + json.dumps(result), flush=True)

    async def session_start(self):
        self.session = await asyncio.to_thread(register, self.state, 'relay', True)

    async def cleanup(self):
        if self.ws:
            await self.ws.close()
        if self.session:
            await self.call('acp_disconnect', {'connectionId': self.session['connectionId']}, 'server')

    async def latency(self):
        await self.session_start()
        result = {}
        try:
            for route in ('server', 'relay'):
                await self.attach(route)
                start = time.monotonic()
                await self.prompt('#stream lines=300 interval_ms=100', route)
                arrivals = []
                while True:
                    event = await self.next_event()
                    if event['type'] == 'content_delta':
                        arrivals.append((time.monotonic()-start)*1000)
                    if event['type'] == 'turn_complete':
                        break
                self.expect_lines(300)
                health = []
                for _ in range(20):
                    begin = time.monotonic()
                    await self.call('health', through=route)
                    health.append((time.monotonic()-begin)*1000)
                result[route] = {'ordered_lines': len(self.lines), 'first_chunk_ms': arrivals[0],
                    'inter_chunk': stats([b-a for a,b in zip(arrivals, arrivals[1:])]),
                    'arrival_ms': arrivals, 'health': stats(health), 'health_samples_ms': health}
                await self.ws.close()
            self.save('scenario2-latency', result)
        finally:
            await self.cleanup()

    async def large(self):
        await self.session_start()
        try:
            await self.prompt('#tools count=250 output_kb=8\n#sleep 30')
            until = time.monotonic()+20
            while time.monotonic() < until:
                raw = await self.call('acp_get_session_snapshot', {'connectionId': self.session['connectionId']}, 'server', True)
                if len(json.loads(raw)['active_tool_calls']) == 250:
                    break
                await asyncio.sleep(.1)
            begin = time.monotonic()
            raw = await self.call('acp_get_session_snapshot', {'connectionId': self.session['connectionId']}, 'relay', True)
            http_ms = (time.monotonic()-begin)*1000
            snap = json.loads(raw)
            self.ws = await WebSocket.connect(self.state['relay_url'], self.state['token'], self.state['fingerprint'])
            await self.ws.send({'action':'attach', 'subscription_id':'large', 'connection_id':self.session['connectionId']})
            begin = time.monotonic()
            frames = []
            while True:
                fin, opcode, payload = await asyncio.wait_for(read_frame(self.ws.reader, False), 15)
                frames.append({'opcode':opcode, 'bytes':len(payload), 'fin':fin})
                if opcode == 9:
                    self.ws.writer.write(frame_bytes(10,payload,True))
                    await self.ws.writer.drain()
                if opcode == 1:
                    msg = json.loads(payload)
                    if msg.get('type') == 'snapshot':
                        break
            ws_ms = (time.monotonic()-begin)*1000
            assert len(raw)>1048576 and len(payload)>1048576
            assert msg['snapshot']['active_tool_calls'] == snap['active_tool_calls']
            assert len(snap['active_tool_calls']) == 250
            self.save('scenario8-snapshot', {'http_bytes':len(raw), 'http_ms':http_ms, 'ws_payload_bytes':len(payload),
                'ws_ms':ws_ms,'tools':250,'tool_payloads_equal':True,'compressed':False, 'frames':frames})
        finally:
            await self.cleanup()

    async def heartbeat(self):
        result = {}
        for route in ('server', 'relay'):
            url, pin = endpoint(self.state, route)
            ws = await WebSocket.connect(url,self.state['token'],pin)
            begin=time.monotonic()
            seen=[]
            try:
                # Passive reader measures exact peer EOF; deliberately NEVER replies to Ping.
                while True:
                    fin,op,payload=await asyncio.wait_for(read_frame(ws.reader,False),50)
                    seen.append({'seconds':time.monotonic()-begin,'opcode':op,'bytes':len(payload)})
                    if op==8:
                        break
            except (EOFError,asyncio.IncompleteReadError,ConnectionError):
                pass
            finally:
                await ws.close()
            elapsed=time.monotonic()-begin
            assert 43<=elapsed<=48, elapsed
            result[route]={'closed_seconds':elapsed,'frames':seen,'pong_sent':0}
            print('heartbeat '+route+' closed at '+str(elapsed),flush=True)
        self.save('scenario4-heartbeat',result)

    async def no_read(self):
        ws=await WebSocket.connect(self.state['server_url'],self.state['token'],None)
        begin=time.monotonic()
        ws.writer.transport.pause_reading()
        await asyncio.sleep(47)
        ws.writer.transport.resume_reading()
        frames=[]
        try:
            while True:
                fin,op,data=await asyncio.wait_for(read_frame(ws.reader,False),2)
                frames.append({'opcode':op,'bytes':len(data)})
                if op==8:break
        except (EOFError,asyncio.IncompleteReadError,ConnectionError):
            pass
        finally:await ws.close()
        elapsed=time.monotonic()-begin
        assert elapsed<49
        self.save('scenario4-no-read',{'no_reads_or_pongs_seconds':47,'eof_observed_seconds':elapsed,'frames':frames})

    async def paired(self):
        await self.session_start()
        sockets=[]
        try:
            for route in ('server','relay'):
                url,pin=endpoint(self.state,route)
                ws=await WebSocket.connect(url,self.state['token'],pin)
                await ws.attach(self.session['connectionId'])
                sockets.append(ws)
            async def collect(ws):
                rows={}
                while True:
                    msg=await ws.receive(40)
                    if msg.get('type')!='event':continue
                    event=msg['envelope']
                    if event['type']=='content_delta':rows[event['text'].strip()]={'seq':event['seq'],'received':time.monotonic()}
                    if event['type']=='turn_complete':return rows
            jobs=[asyncio.create_task(collect(ws)) for ws in sockets]
            await self.prompt('#stream lines=300 interval_ms=100')
            direct,relay=await asyncio.gather(*jobs)
            expected=[f'line {i:06d}' for i in range(1,301)]
            assert list(direct)==list(relay)==expected
            assert all(direct[k]['seq']==relay[k]['seq'] for k in expected)
            deltas=[(relay[k]['received']-direct[k]['received'])*1000 for k in expected]
            self.save('scenario2-paired',{'lines':300,'same_seq':True,'relay_minus_direct_ms':stats(deltas),'samples_ms':deltas})
        finally:
            for ws in sockets:await ws.close()
            await self.cleanup()

    async def restart(self):
        await self.session_start()
        try:
            await self.attach()
            await self.prompt('#stream lines=120 interval_ms=1000')
            while len(self.lines)<3:
                await self.next_event()
            cursor=self.seq
            await self.ws.close()
            begin=time.monotonic()
            cmd=[sys.executable,'e2e/environment.py','restart-relay','--name',self.state['name'],'--downtime','10']
            process=await asyncio.create_subprocess_exec(*cmd,env=clean_env(),stdout=asyncio.subprocess.PIPE,stderr=asyncio.subprocess.STDOUT)
            states=[]
            while process.returncode is None:
                states.append(await self.call('get_relay_status',through='server'))
                try:
                    await asyncio.wait_for(asyncio.shield(process.wait()),.2)
                except TimeoutError:
                    pass
            output=(await process.stdout.read()).decode()
            assert process.returncode==0,output
            first=await self.attach('relay',cursor)
            assert first['type']=='replay', first['type']
            await self.until_complete()
            self.expect_lines(120)
            self.save('scenario5-restart',{'cursor':cursor,'attach_type':first['type'],'replayed':len(first['events']),
                'lines':len(self.lines),'gaps':0,'duplicates':0,'restart_output':output,'status_samples':states,
                'total_seconds':time.monotonic()-begin})
        finally:
            await self.cleanup()

    async def pressure(self):
        await self.session_start()
        await asyncio.to_thread(control,self.state,'reset')
        await self.attach('proxy')
        result={'bps':20000,'observations':[],'frames':[],'pongs':[], 'ping_interval_s':2}
        start=time.monotonic()
        second_sent=False
        total_bytes=0
        last_progress=start
        stop_event=asyncio.Event()

        async def sender():
            nonlocal second_sent
            await asyncio.sleep(.1)
            await self.ws.send({'action':'ping'})
            await self.ws.send({'action':'attach','subscription_id':'pressure-second','connection_id':self.session['connectionId']})
            second_sent=True
            while not stop_event.is_set():
                await self.ws.send({'action':'ping'})
                await asyncio.sleep(2)

        async def read_exact(n):
            nonlocal total_bytes,last_progress
            buf=bytearray()
            while len(buf)<n:
                data=await self.ws.reader.read(min(16384,n-len(buf)))
                if not data: raise EOFError('upstream EOF')
                total_bytes+=len(data)
                last_progress=time.monotonic()
                buf.extend(data)
            return bytes(buf)

        async def reader():
            while True:
                head=await read_exact(2)
                size=head[1]&127
                if size==126: size=struct.unpack('!H',await read_exact(2))[0]
                elif size==127:size=struct.unpack('!Q',await read_exact(8))[0]
                payload=await read_exact(size)
                op=head[0]&15
                t=time.monotonic()-start
                row={'s':t,'op':op,'bytes':size}
                if op==1:
                    msg=json.loads(payload)
                    row.update(type=msg.get('type'),subscription=msg.get('subscription_id'))
                    if msg.get('type')=='pong':result['pongs'].append(t)
                if op==9:
                    self.ws.writer.write(frame_bytes(10,payload,True))
                    await self.ws.writer.drain()
                result['frames'].append(row)
                if op==8:raise EOFError('close frame')

        jobs=[]
        try:
            await asyncio.to_thread(control,self.state,'throttle',bps=20000)
            await self.prompt('#tools count=250 output_kb=8\n#sleep 70')
            start=time.monotonic()
            jobs=[asyncio.create_task(sender()),asyncio.create_task(reader())]
            for _ in range(55):
                status=await self.call('get_relay_status',through='server')
                result['observations'].append({'s':time.monotonic()-start,'active_streams':status['active_streams'],
                    'state':status['state'],'bytes_received':total_bytes,'silence_s':time.monotonic()-last_progress})
                if jobs[1].done():
                    result['reader_end']=repr(jobs[1].exception())
                    result['reader_end_observed_s']=time.monotonic()-start
                    break
                await asyncio.sleep(1)
            result['second_attach_sent']=second_sent
            result['fault_status']=await asyncio.to_thread(control,self.state,'status')
            result['max_byte_silence_s']=max(x['silence_s'] for x in result['observations'])
            seen_active=False
            result['closed_upstream_at_s']=None
            for observation in result['observations']:
                if observation['active_streams']>0:seen_active=True
                elif seen_active:
                    result['closed_upstream_at_s']=observation['s']
                    break
            result['received_bytes']=total_bytes
            # Remove throttle to drain queued bytes and distinguish buffered data from silence.
            await asyncio.to_thread(control,self.state,'reset')
            await asyncio.sleep(3)
            result['after_reset_reader_end']=repr(jobs[1].exception()) if jobs[1].done() else 'responsive reader still running'
            result['observed_seconds']=time.monotonic()-start
            self.save('scenario3-pressure',result)
            # The 30 s timer starts at a blocked sink.send, NOT at prompt start.
            # Paired server debug logs identify which deadline actually ended it.
            assert result['closed_upstream_at_s'] is not None or result['pongs'], 'neither close nor app Pong observed'
            assert result['max_byte_silence_s']<45
        finally:
            stop_event.set()
            for job in jobs: job.cancel()
            await asyncio.gather(*jobs,return_exceptions=True)
            await asyncio.to_thread(control,self.state,'reset')
            await self.cleanup()


async def main():
    parser=argparse.ArgumentParser()
    parser.add_argument('--name',default='e1a')
    parser.add_argument('--evidence',required=True)
    parser.add_argument('scenario',choices=['latency','large','heartbeat','no_read','paired','restart','pressure'])
    args=parser.parse_args()
    await getattr(Verify(load_state(args.name),args.evidence),args.scenario)()


if __name__=='__main__':asyncio.run(main())
