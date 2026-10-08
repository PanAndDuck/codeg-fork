#!/usr/bin/env python3
"""Run E1b's three card recovery checks in the already-open session 1."""
import time

from ios_scenarios import call, capture, fault, finished, record, send, wait_ui


def main():
    for directive, field, label in (
        ('#permission', 'pending_permission', 'Allow once'),
        ('#question', 'pending_question', 'Mock choice?'),
        ('#approve_plan', 'pending_plan_approval', 'Approve & Build'),
    ):
        name = 's14-' + directive[1:]
        send(directive + '\n#echo ' + name + '_DONE', name)
        connection = call('acp_find_connection_for_conversation', {
            'conversationId': 1, 'agentType': 'custom:e2e-mock'})['connection_id']
        deadline = time.monotonic() + 15
        while True:
            snapshot = call('acp_get_session_snapshot', {'connectionId': connection})
            if snapshot.get(field):
                break
            if time.monotonic() > deadline:
                raise RuntimeError('card did not become pending: ' + field)
            time.sleep(.2)
        capture(name + '-pending')
        fault('omit_since_seq', count=1)
        fault('drop_ws')
        time.sleep(3)
        restored = capture(name + '-pending-restored')
        fault('block_ws', seconds=30)
        pending = snapshot[field]
        params = {'connectionId': connection}
        if field == 'pending_permission':
            command = 'acp_respond_permission'
            params.update(requestId=pending['request_id'], optionId='allow_once')
        elif field == 'pending_question':
            command = 'acp_answer_question'
            params.update(questionId=pending['question_id'], answer={'answers': [{
                'questionId': pending['questions'][0]['id'], 'labels': ['Alpha']}]})
        else:
            command = 'acp_answer_plan_approval'
            params.update(approvalId=pending['approval_id'], answer={'decision': 'approve', 'feedback': ''})
        call(command, params)
        capture(name + '-resolved-during-gap')
        fault('reset')
        result = wait_ui(name + '-finished', lambda tree: finished(tree, name + '_DONE'), 45)
        tree = capture(name + '-cleared')
        result.update(pending_restored=label in restored, card_cleared=label not in tree)
        record(name, result)


if __name__ == '__main__':
    main()
